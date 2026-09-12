use std::path::PathBuf;
use std::process::Command;

use k8s_openapi::api::core::v1::Namespace;
use kube::{
    Api, Client,
    api::{DeleteParams, ListParams, Patch, PatchParams},
};
use ws_proxy::kubernetes::{
    self, Error,
    resources::{SuntitleHost, SuntitleNode},
};

#[allow(dead_code)]
pub fn require_free_ports() -> std::io::Result<()> {
    let _proxy = std::net::TcpListener::bind("127.0.0.1:3030")?;
    let _api = std::net::TcpListener::bind("127.0.0.1:3031")?;
    Ok(())
}

pub struct TestNamespace {
    name: String,
    client: Client,
    root: PathBuf,
}

impl TestNamespace {
    pub async fn new() -> Result<Self, Error> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned();
        let client = kubernetes::client_from_kubeconfig(&root.join("pki/admin.kubeconfig")).await?;
        let namespace = Self {
            name: format!("e2e-{}", uuid::Uuid::new_v4()),
            client,
            root,
        };
        kubernetes::bootstrap(namespace.client.clone(), &namespace.name).await?;
        Ok(namespace)
    }

    pub fn command(&self, binary: PathBuf, role: &str) -> Command {
        let mut command = Command::new(binary);
        command
            .arg("--kubeconfig")
            .arg(self.root.join(format!("pki/suntitle-{role}.kubeconfig")))
            .arg("--namespace")
            .arg(&self.name);
        command
    }

    async fn cleanup(client: Client, name: String) -> Result<(), Error> {
        let nodes: Api<SuntitleNode> = Api::namespaced(client.clone(), &name);
        let hosts: Api<SuntitleHost> = Api::namespaced(client.clone(), &name);
        let patch = Patch::Merge(serde_json::json!({"metadata":{"finalizers":[]}}));
        for node in nodes.list(&ListParams::default()).await? {
            let name = node.metadata.name.unwrap();
            nodes.patch(&name, &PatchParams::default(), &patch).await?;
            nodes.delete(&name, &DeleteParams::default()).await?;
        }
        for host in hosts.list(&ListParams::default()).await? {
            let name = host.metadata.name.unwrap();
            hosts.patch(&name, &PatchParams::default(), &patch).await?;
            hosts.delete(&name, &DeleteParams::default()).await?;
        }
        for plural in ["roles", "rolebindings"] {
            let resource = kube::api::ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
                "rbac.authorization.k8s.io",
                "v1",
                if plural == "roles" {
                    "Role"
                } else {
                    "RoleBinding"
                },
            ));
            let api: Api<kube::api::DynamicObject> =
                Api::namespaced_with(client.clone(), &name, &resource);
            api.delete_collection(&DeleteParams::default(), &ListParams::default())
                .await?;
        }
        let namespaces: Api<Namespace> = Api::all(client);
        namespaces.delete(&name, &DeleteParams::default()).await?;
        // The standalone API has no namespace controller. Tests MUST finalize their own namespace.
        let mut namespace = namespaces.get(&name).await?;
        namespace.spec.as_mut().unwrap().finalizers = None;
        namespaces
            .replace_subresource("finalize", &name, &Default::default(), &namespace)
            .await?;
        Ok(())
    }
}

impl Drop for TestNamespace {
    fn drop(&mut self) {
        let client = self.client.clone();
        let name = self.name.clone();
        let result = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()?.block_on(Self::cleanup(client, name))
        })
        .join();
        if !matches!(result, Ok(Ok(()))) {
            eprintln!("Failed to clean up E2E namespace {}: {result:?}", self.name);
        }
    }
}
