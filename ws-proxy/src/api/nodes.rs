use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::kubernetes::{
    resources::SuntitleNode,
    store::{NodeUpdate, ResourceList, ResourceStore},
};
use actix_web::{HttpResponse, delete, get, post, put, web};

#[utoipa::path(get, path = "/nodes", tag = "Nodes", operation_id = "listNodes",
    responses((status = 200, body = ResourceList<SuntitleNode>), (status = 503, body = ErrorEnvelope)))]
#[get("/nodes")]
pub async fn list_nodes(store: web::Data<ResourceStore>) -> ApiResult<ResourceList<SuntitleNode>> {
    Ok(web::Json(store.list_nodes().await?))
}

#[utoipa::path(get, path = "/nodes/{name}", tag = "Nodes", operation_id = "getNode",
    params(("name" = String, Path)),
    responses((status = 200, body = SuntitleNode), (status = 404, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[get("/nodes/{name}")]
pub async fn get_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> ApiResult<SuntitleNode> {
    Ok(web::Json(store.node(&name).await?))
}

#[utoipa::path(put, path = "/nodes/{name}", tag = "Nodes", operation_id = "updateNode",
    description = "Persist a complete desired spec without a resourceVersion precondition. Status is preserved. Success does not imply runtime application.",
    params(("name" = String, Path)), request_body = NodeUpdate,
    responses((status = 200, body = SuntitleNode), (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[put("/nodes/{name}")]
pub async fn update_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
    body: web::Json<NodeUpdate>,
) -> ApiResult<SuntitleNode> {
    Ok(web::Json(store.update(&name, body.into_inner()).await?))
}

#[utoipa::path(post, path = "/nodes/{name}/restart", tag = "Nodes", operation_id = "restartNode",
    description = "Persist a fresh restart token. Runtime completion is asynchronous; pending requests may coalesce.",
    params(("name" = String, Path)),
    responses((status = 202, body = SuntitleNode), (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/nodes/{name}/restart")]
pub async fn restart_node(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Accepted().json(store.restart(&name).await?))
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
