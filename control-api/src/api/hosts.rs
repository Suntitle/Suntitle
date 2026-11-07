//! Desired writes persist before runtime reconciliation. Reads pass through resource status.

use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use super::nodes::Node;
use crate::store::{NodeCreate, ResourceList, ResourceStore};
use actix_web::{HttpResponse, get, post, web};
use suntitle_core::kubernetes::resources::{HostStatus, SuntitleHost, SuntitleHostSpec};

/// Public host identity, flat desired configuration, and unchanged resource status.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Host {
    /// Resource name used in API URLs and references.
    pub name: String,
    /// Desired fields exposed at the response's top level.
    #[serde(flatten)]
    pub spec: SuntitleHostSpec,
    /// Resource status reported asynchronously by the controller.
    pub status: Option<HostStatus>,
}

impl From<SuntitleHost> for Host {
    /// Extract identity and spec without interpreting resource status.
    fn from(resource: SuntitleHost) -> Self {
        use kube::ResourceExt;
        Self {
            name: resource.name_any(),
            spec: resource.spec,
            status: resource.status,
        }
    }
}

/// List flat resources with the collection revision.
#[utoipa::path(get, path = "/hosts", tag = "Hosts", operation_id = "listHosts",
    responses((status = 200, body = ResourceList<Host>), (status = 503, body = ErrorEnvelope)))]
#[get("/hosts")]
pub async fn list_hosts(store: web::Data<ResourceStore>) -> ApiResult<ResourceList<Host>> {
    let list = store.list_hosts().await?;
    Ok(web::Json(ResourceList {
        items: list.items.into_iter().map(Host::from).collect(),
        resource_version: list.resource_version,
    }))
}

/// Persist a node assigned to the host selected by resource name.
#[utoipa::path(post, path = "/hosts/{name}/nodes", tag = "Nodes", operation_id = "installNodeOnHost",
    description = "Persist desired state with host ownership. The path selects the host by resource name. Success does not imply runtime execution.",
    params(("name" = String, Path)), request_body = NodeCreate,
    responses((status = 201, body = Node, headers(("Location" = String, description = "Created resource URL"))),
        (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/hosts/{name}/nodes")]
pub async fn install_node_on_host(
    store: web::Data<ResourceStore>,
    host: web::Path<String>,
    body: web::Json<NodeCreate>,
) -> Result<HttpResponse, ApiError> {
    let node = Node::from(store.create(&host, body.into_inner()).await?);
    Ok(HttpResponse::Created()
        .insert_header(("Location", format!("/api/v1/nodes/{}", node.name)))
        .json(node))
}
