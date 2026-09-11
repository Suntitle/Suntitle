use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::{
    Api, Client, ResourceExt,
    api::{DeleteParams, ListParams, Patch, PatchParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::resources::*;
use crate::api::errors::ApiError;

#[derive(Clone)]
pub struct ResourceStore {
    client: Client,
    hosts: Api<SuntitleHost>,
    nodes: Api<SuntitleNode>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostList {
    pub items: Vec<SuntitleHost>,
    pub resource_version: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeList {
    pub items: Vec<SuntitleNode>,
    pub resource_version: String,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeCreate {
    pub name: Option<String>,
    pub spec: SuntitleNodeSpec,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeUpdate {
    pub spec: SuntitleNodeSpec,
}

impl ResourceStore {

    pub fn new(client: Client, namespace: &str) -> Self {
        Self {
            hosts: Api::namespaced(client.clone(), namespace),
            nodes: Api::namespaced(client.clone(), namespace),
            client,
        }
    }

    pub async fn ready(&self) -> Result<(), ApiError> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        for name in [
            "suntitlehosts.control.suntitle.io",
            "suntitlenodes.control.suntitle.io",
        ] {
            let crd = crds.get(name).await?;
            if !crd
                .status
                .and_then(|s| s.conditions)
                .unwrap_or_default()
                .iter()
                .any(|c| c.type_ == "Established" && c.status == "True")
            {
                return Err(ApiError::unavailable());
            }
        }
        self.hosts.list(&ListParams::default().limit(1)).await?;
        self.nodes.list(&ListParams::default().limit(1)).await?;
        Ok(())
    }

    pub async fn list_hosts(&self) -> Result<HostList, ApiError> {
        let list = self.hosts.list(&ListParams::default()).await?;
        Ok(HostList {
            items: list.items,
            resource_version: list
                .metadata
                .resource_version
                .ok_or_else(ApiError::unavailable)?,
        })
    }

    pub async fn list_nodes(&self) -> Result<NodeList, ApiError> {
        let list = self.nodes.list(&ListParams::default()).await?;
        Ok(NodeList {
            items: list.items,
            resource_version: list
                .metadata
                .resource_version
                .ok_or_else(ApiError::unavailable)?,
        })
    }

    pub async fn node(&self, name: &str) -> Result<SuntitleNode, ApiError> {
        self.nodes.get(name).await.map_err(ApiError::from)
    }

    pub async fn create(
        &self,
        host_uid: &str,
        request: NodeCreate,
    ) -> Result<SuntitleNode, ApiError> {
        let host = self
            .hosts
            .list(&ListParams::default())
            .await?
            .items
            .into_iter()
            .find(|host| host.metadata.uid.as_deref() == Some(host_uid))
            .ok_or_else(|| ApiError::not_found(format!("Host UID {host_uid} was not found.")))?;
        let name = request
            .name
            .unwrap_or_else(|| format!("node-{}", Uuid::new_v4()));
        let mut node = SuntitleNode::new(&name, request.spec);
        node.spec.host_ref.name = host.name_any();
        Ok(self.nodes.create(&PostParams::default(), &node).await?)
    }

    pub async fn update(&self, name: &str, request: NodeUpdate) -> Result<SuntitleNode, ApiError> {
        let patch = json_patch::Patch(vec![json_patch::PatchOperation::Replace(
            json_patch::ReplaceOperation {
                path: "/spec".parse().expect("static JSON pointer"),
                value: json!(request.spec),
            },
        )]);
        Ok(self
            .nodes
            .patch(name, &PatchParams::default(), &Patch::<()>::Json(patch))
            .await?)
    }

    /// Cleanup completes asynchronously through the node finalizer.
    pub async fn delete(&self, name: &str) -> Result<(), ApiError> {
        self.nodes.delete(name, &DeleteParams::default()).await?;
        Ok(())
    }

    pub async fn restart(&self, name: &str) -> Result<SuntitleNode, ApiError> {
        let patch = json!({"spec": {"restartToken": Uuid::new_v4().to_string()}});
        Ok(self
            .nodes
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?)
    }
}
