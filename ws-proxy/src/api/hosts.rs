use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::kubernetes::{
    resources::SuntitleNode,
    store::{HostList, NodeCreate, ResourceStore},
};
use actix_web::{HttpResponse, get, post, web};
use kube::ResourceExt;

#[utoipa::path(get, path = "/hosts", tag = "Hosts", operation_id = "listHosts",
    responses((status = 200, body = HostList), (status = 503, body = ErrorEnvelope)))]
#[get("/hosts")]
pub async fn list_hosts(store: web::Data<ResourceStore>) -> ApiResult<HostList> {
    Ok(web::Json(store.list_hosts().await?))
}

#[utoipa::path(post, path = "/hosts/{hostName}/nodes", tag = "Nodes", operation_id = "installNodeOnHost",
    description = "Persist desired state with host ownership. The path selects the host. Success does not imply runtime execution.",
    params(("hostName" = String, Path)), request_body = NodeCreate,
    responses((status = 201, body = SuntitleNode, headers(("Location" = String, description = "Created resource URL"))),
        (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/hosts/{hostName}/nodes")]
pub async fn install_node_on_host(
    store: web::Data<ResourceStore>,
    host: web::Path<String>,
    body: web::Json<NodeCreate>,
) -> Result<HttpResponse, ApiError> {
    let node = store.create(&host, body.into_inner()).await?;
    Ok(HttpResponse::Created()
        .insert_header(("Location", format!("/api/v1/nodes/{}", node.name_any())))
        .json(node))
}
