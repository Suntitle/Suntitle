//! Kubernetes client, resource storage, and bootstrap.
pub mod adapter;
pub mod execution;
pub mod resources;
pub mod store;

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

/// Explicit credentials avoid accidentally installing resources in the current context.
pub async fn client_from_kubeconfig(path: &Path) -> Result<Client, Error> {
    let mut config =
        Config::from_custom_kubeconfig(Kubeconfig::read_from(path)?, &KubeConfigOptions::default())
            .await?;
    config.default_retry = false;
    config.connect_timeout = Some(Duration::from_secs(5));
    config.read_timeout = Some(Duration::from_secs(10));
    Ok(Client::try_from(config)?)
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
