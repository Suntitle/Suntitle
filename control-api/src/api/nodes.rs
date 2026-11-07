//! Desired writes persist before runtime reconciliation. Reads pass through resource status.

use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::store::{NodeUpdate, ResourceList, ResourceStore};
use actix_web::{HttpResponse, delete, get, post, put, web};
use suntitle_core::kubernetes::resources::{NodeStatus, SuntitleNode, SuntitleNodeSpec};

/// Public node identity, flat desired configuration, and unchanged resource status.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Node {
    /// Resource name used in API URLs and references.
    pub name: String,
    /// Desired fields exposed at the response's top level.
    #[serde(flatten)]
    pub spec: SuntitleNodeSpec,
    /// Resource status reported asynchronously by the controller.
    pub status: Option<NodeStatus>,
}

impl From<SuntitleNode> for Node {
    /// Extract identity and spec without interpreting resource status.
    fn from(resource: SuntitleNode) -> Self {
        use kube::ResourceExt;
        Self {
            name: resource.name_any(),
            spec: resource.spec,
            status: resource.status,
        }
    }
}

/// List flat resources with the collection revision.
#[utoipa::path(get, path = "/nodes", tag = "Nodes", operation_id = "listNodes",
    responses((status = 200, body = ResourceList<Node>), (status = 503, body = ErrorEnvelope)))]
#[get("/nodes")]
pub async fn list_nodes(store: web::Data<ResourceStore>) -> ApiResult<ResourceList<Node>> {
    let list = store.list_nodes().await?;
    Ok(web::Json(ResourceList {
        items: list.items.into_iter().map(Node::from).collect(),
        resource_version: list.resource_version,
    }))
}

/// Read the named node and its resource status.
#[utoipa::path(get, path = "/nodes/{name}", tag = "Nodes", operation_id = "getNode",
    params(("name" = String, Path)),
    responses((status = 200, body = Node), (status = 404, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[get("/nodes/{name}")]
pub async fn get_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> ApiResult<Node> {
    Ok(web::Json(store.node(&name).await?.into()))
}

/// Persist flat desired configuration; runtime application is asynchronous.
#[utoipa::path(put, path = "/nodes/{name}", tag = "Nodes", operation_id = "updateNode",
    description = "Persist a complete desired spec without a resourceVersion precondition. Status is preserved. Success does not imply runtime application.",
    params(("name" = String, Path)), request_body = NodeUpdate,
    responses((status = 200, body = Node), (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[put("/nodes/{name}")]
pub async fn update_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
    body: web::Json<NodeUpdate>,
) -> ApiResult<Node> {
    Ok(web::Json(store.update(&name, body.into_inner()).await?.into()))
}

/// Request a restart and return the resource status unchanged.
#[utoipa::path(post, path = "/nodes/{name}/restart", tag = "Nodes", operation_id = "restartNode",
    description = "Persist a fresh restart token. Runtime completion is asynchronous; pending requests may coalesce.",
    params(("name" = String, Path)),
    responses((status = 202, body = Node), (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/nodes/{name}/restart")]
pub async fn restart_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Accepted().json(Node::from(store.restart(&name).await?)))
}

#[utoipa::path(delete, path = "/nodes/{name}", tag = "Nodes", operation_id = "deleteNode",
    description = "Request asynchronous removal. Terminating resources remain visible until runtime cleanup is acknowledged, including while the host is offline.",
    params(("name" = String, Path)),
    responses((status = 202, description = "Deletion requested"), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[delete("/nodes/{name}")]
pub async fn delete_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    store.delete(&name).await?;
    Ok(HttpResponse::Accepted().finish())
}
