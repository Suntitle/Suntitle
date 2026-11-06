//! Shared E2E infrastructure. Recipes prepare binaries before harness startup.
//! Application guards MUST stop children before namespace and temporary-file cleanup.
#![allow(dead_code)]

mod api;
mod process;

pub use api::ApiClient;
#[allow(unused_imports)]
pub use process::{ChildProcess, binary_directory, binary_path, fixture_python, wait_for_services};

use std::path::PathBuf;
use std::process::Command;

use k8s_openapi::api::core::v1::Namespace;
use kube::{
    Api, Client,
    api::{DeleteParams, ListParams, Patch, PatchParams},
};
use suntitle_core::kubernetes::{
    self, Error,
    resources::{SuntitleExtension, SuntitleHost, SuntitleNode},
};

/// Require exclusive local API and proxy ports before starting application children.
pub fn require_free_ports() -> std::io::Result<()> {
    let _proxy = std::net::TcpListener::bind("127.0.0.1:3030")?;
    let _api = std::net::TcpListener::bind("127.0.0.1:3031")?;
    Ok(())
}

/// Isolated resources and credentials owned by one E2E run.
pub struct TestNamespace {
    /// Namespace removed when this guard is dropped.
    name: String,
    /// Bootstrap client used to create and remove test resources.
    client: Client,
    /// Directory containing explicit admin, API, and proxy kubeconfigs.
    kubeconfigs: PathBuf,
}

impl TestNamespace {
    /// Bootstrap against the normal local Kubernetes endpoint.
    pub async fn new() -> Result<Self, Error> {
        let kubeconfigs = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("pki");
        Self::with_kubeconfigs(kubeconfigs).await
    }

    /// Bootstrap against an explicitly selected test Kubernetes server.
    pub async fn with_kubeconfigs(kubeconfigs: PathBuf) -> Result<Self, Error> {
        let client =
            kubernetes::client_from_kubeconfig(&kubeconfigs.join("admin.kubeconfig")).await?;
        let namespace = Self {
            name: format!("e2e-{}", uuid::Uuid::new_v4()),
            client,
            kubeconfigs,
        };
        kubernetes::bootstrap(namespace.client.clone(), &namespace.name).await?;
        Ok(namespace)
    }

    /// Configure one application with namespace-scoped test credentials.
    pub fn command(&self, binary: PathBuf, role: &str) -> Command {
        let mut command = Command::new(binary);
        command
            .arg("--kubeconfig")
            .arg(self.kubeconfigs.join(format!("suntitle-{role}.kubeconfig")))
            .arg("--namespace")
            .arg(&self.name);
        command
    }

    /// Access node observations in this test's isolated namespace.
    pub fn nodes(&self) -> Api<SuntitleNode> {
        Api::namespaced(self.client.clone(), &self.name)
    }

    /// Remove owned resources before finalizing a namespace without a namespace controller.
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
        let extensions: Api<SuntitleExtension> = Api::namespaced(client.clone(), &name);
        extensions
            .delete_collection(&DeleteParams::default(), &ListParams::default())
            .await?;
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
