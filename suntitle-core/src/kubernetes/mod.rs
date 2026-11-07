//! Shared Kubernetes resources, client configuration, and bootstrap.
//! Request clients bound response reads. Controller clients defer idle detection
//! to the watcher, which reconnects interrupted streams and resumes observation.
pub mod adapter;
pub mod execution;
pub mod resources;

use k8s_openapi::{
    api::core::v1::Namespace,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
};
use kube::{
    Api, Client, Config, ResourceExt,
    api::{ApiResource, DynamicObject, Patch, PatchParams},
    config::{KubeConfigOptions, Kubeconfig},
    runtime::wait::{await_condition, conditions},
};
use serde::Deserialize;
use serde_json::Value;
use std::{path::Path, time::Duration};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Build a request client with explicit credentials and bounded response reads.
pub async fn client_from_kubeconfig(path: &Path) -> Result<Client, Error> {
    let config = config_from_kubeconfig(path, Some(Duration::from_secs(10))).await?;
    Ok(Client::try_from(config)?)
}

/// Build a controller client; watcher-level timeouts own idle stream recovery.
pub async fn watch_client_from_kubeconfig(path: &Path) -> Result<Client, Error> {
    let config = config_from_kubeconfig(path, None).await?;
    Ok(Client::try_from(config)?)
}

/// Keep credentials, connection deadlines, and mutation retry policy shared across clients.
async fn config_from_kubeconfig(
    path: &Path,
    read_timeout: Option<Duration>,
) -> Result<Config, Error> {
    let mut config =
        Config::from_custom_kubeconfig(Kubeconfig::read_from(path)?, &KubeConfigOptions::default())
            .await?;
    config.default_retry = false;
    config.connect_timeout = Some(Duration::from_secs(5));
    config.read_timeout = read_timeout;
    Ok(config)
}

pub async fn apply_crds(client: Client) -> Result<(), Error> {
    let api: Api<CustomResourceDefinition> = Api::all(client);
    let params = PatchParams::apply("suntitle-bootstrap");
    for crd in resources::crds() {
        let name = crd.name_any();
        api.patch(&name, &params, &Patch::Apply(&crd)).await?;
        tokio::time::timeout(
            Duration::from_secs(60),
            await_condition(api.clone(), &name, conditions::is_crd_established()),
        )
        .await??;
    }
    Ok(())
}

/// Idempotent installation with bootstrap credentials; never force SSA conflicts.
pub async fn bootstrap(client: Client, namespace: &str) -> Result<(), Error> {
    let params = PatchParams::apply("suntitle-bootstrap");
    let mut ns: Namespace =
        serde_yaml::from_str(include_str!("../../../kubernetes/namespace.yaml"))?;
    ns.metadata.name = Some(namespace.to_owned());
    Api::<Namespace>::all(client.clone())
        .patch(namespace, &params, &Patch::Apply(&ns))
        .await?;
    apply_crds(client.clone()).await?;
    for document in
        serde_yaml::Deserializer::from_str(include_str!("../../../kubernetes/rbac.yaml"))
    {
        let mut value = Value::deserialize(document)?;
        if !value["kind"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Cluster")
        {
            value["metadata"]["namespace"] = Value::String(namespace.to_owned());
        }
        let object: DynamicObject = serde_json::from_value(value)?;
        let kind = &object
            .types
            .as_ref()
            .ok_or("RBAC object missing type")?
            .kind;
        let resource = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
            "rbac.authorization.k8s.io",
            "v1",
            kind,
        ));
        let api: Api<DynamicObject> = if kind.starts_with("Cluster") {
            Api::all_with(client.clone(), &resource)
        } else {
            Api::namespaced_with(client.clone(), namespace, &resource)
        };
        api.patch(&object.name_any(), &params, &Patch::Apply(&object))
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use futures_util::TryStreamExt;
    use kube::api::{WatchEvent, WatchParams};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Quiet watches MUST survive the request client's ten-second transport deadline.
    #[tokio::test]
    async fn controller_watch_survives_more_than_ten_seconds_without_events() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                request.extend_from_slice(&byte);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            assert!(String::from_utf8_lossy(&request).contains("watch=true"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_secs(11)).await;
            let event = serde_json::json!({"type":"ADDED","object":{"apiVersion":"control.suntitle.io/v1alpha1","kind":"SuntitleHost","metadata":{"name":"quiet-host","resourceVersion":"2"},"spec":{"displayName":"Quiet host"}}}).to_string() + "\n";
            socket
                .write_all(format!("{:x}\r\n{}\r\n0\r\n\r\n", event.len(), event).as_bytes())
                .await
                .unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kubeconfig.yaml");
        std::fs::write(&path, format!("apiVersion: v1\nkind: Config\nclusters:\n- name: test\n  cluster:\n    server: http://{address}\nusers:\n- name: test\n  user: {{}}\ncontexts:\n- name: test\n  context:\n    cluster: test\n    user: test\ncurrent-context: test\n")).unwrap();
        let client = watch_client_from_kubeconfig(&path).await.unwrap();
        let api = Api::<resources::SuntitleHost>::namespaced(client, "test");
        let stream = api
            .watch(&WatchParams::default().timeout(30), "0")
            .await
            .unwrap();
        tokio::pin!(stream);
        let event = tokio::time::timeout(Duration::from_secs(20), stream.try_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match event {
            WatchEvent::Added(host) => assert_eq!(host.name_any(), "quiet-host"),
            other => panic!("expected host event after idle period, received {other:?}"),
        }
        server.await.unwrap();
    }
}
