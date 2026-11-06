use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::{
    Api, Client, Resource as _, ResourceExt,
    api::{DeleteParams, ListParams, Patch, PatchParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::errors::ApiError;
use suntitle_core::kubernetes::resources::*;

#[derive(Clone)]
pub struct ResourceStore {
    client: Client,
    hosts: Api<SuntitleHost>,
    nodes: Api<SuntitleNode>,
    extensions: Api<SuntitleExtension>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceList<T> {
    #[schema(schema_with = resource_items_schema::<T>)]
    pub items: Vec<T>,
    pub resource_version: String,
}

fn resource_items_schema<T: ToSchema>() -> utoipa::openapi::schema::Array {
    utoipa::openapi::schema::ArrayBuilder::new()
        .items(utoipa::openapi::Ref::from_schema_name(T::name()))
        .build()
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtensionCreate {
    pub name: Option<String>,
    pub spec: SuntitleExtensionSpec,
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
            extensions: Api::namespaced(client.clone(), namespace),
            client,
        }
    }

    pub async fn ready(&self) -> Result<(), ApiError> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        for name in [
            "suntitlehosts.control.suntitle.io",
            "suntitlenodes.control.suntitle.io",
            "suntitleextensions.control.suntitle.io",
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
        self.extensions
            .list(&ListParams::default().limit(1))
            .await?;
        Ok(())
    }

    pub async fn list_extensions(&self) -> Result<ResourceList<SuntitleExtension>, ApiError> {
        let list = self.extensions.list(&ListParams::default()).await?;
        Ok(ResourceList {
            items: list.items,
            resource_version: list
                .metadata
                .resource_version
                .ok_or_else(ApiError::unavailable)?,
        })
    }

    pub async fn extension(&self, name: &str) -> Result<SuntitleExtension, ApiError> {
        Ok(self.extensions.get(name).await?)
    }

    pub async fn create_extension(
        &self,
        request: ExtensionCreate,
    ) -> Result<SuntitleExtension, ApiError> {
        let name = request
            .name
            .unwrap_or_else(|| format!("extension-{}", Uuid::new_v4()));
        let extension = SuntitleExtension::new(&name, request.spec);
        Ok(self
            .extensions
            .create(&PostParams::default(), &extension)
            .await?)
    }

    pub async fn refresh_extension(&self, name: &str) -> Result<SuntitleExtension, ApiError> {
        let patch = json!({"spec": {"refreshToken": Uuid::new_v4().to_string()}});
        Ok(self
            .extensions
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?)
    }

    pub async fn list_hosts(&self) -> Result<ResourceList<SuntitleHost>, ApiError> {
        let list = self.hosts.list(&ListParams::default()).await?;
        Ok(ResourceList {
            items: list.items,
            resource_version: list
                .metadata
                .resource_version
                .ok_or_else(ApiError::unavailable)?,
        })
    }

    pub async fn list_nodes(&self) -> Result<ResourceList<SuntitleNode>, ApiError> {
        let list = self.nodes.list(&ListParams::default()).await?;
        Ok(ResourceList {
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
        node.metadata.owner_references = host.controller_owner_ref(&()).map(|owner| vec![owner]);
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
