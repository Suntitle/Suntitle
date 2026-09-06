use super::validated_json::ValidatedJson;
use actix_web::{HttpResponse, get, post, web};
use uuid::Uuid;

use crate::storage::Store;

use super::errors::{ApiResult, BadRequest, NotFound, Unauthorized, ValidationFailed};
use super::models::{HostList, ListHostsQuery, Node, NodeCreate};

#[utoipa::path(
    get,
    path = "/hosts",
    tag = "Hosts",
    operation_id = "listHosts",
    summary = "List hosts",
    description = "Returns hosts running one or more pipeline nodes, including their IP and software version.",
    params(ListHostsQuery),
    responses(
        (status = 200, description = "Known hosts.", body = HostList),
        (status = 401, response = Unauthorized)
    )
)]
#[get("/hosts")]
pub async fn list_hosts(
    store: web::Data<Store>,
    query: web::Query<ListHostsQuery>,
) -> ApiResult<HostList> {
    let mut items: Vec<_> = store
        .hosts()
        .await?
        .into_iter()
        .filter(|host| query.status.is_none_or(|status| host.status == status))
        .collect();
    items.sort_unstable_by_key(|host| host.id);

    Ok(web::Json(HostList {
        total: items.len() as u64,
        items,
    }))
}

#[utoipa::path(
    post,
    path = "/hosts/{hostId}/nodes",
    tag = "Nodes",
    operation_id = "installNodeOnHost",
    summary = "Install a node on a host",
    description = "Installs a configured pipeline node on the selected host. The request `type` and `settings` must match one of the definitions returned by `GET /node-types`.",
    params(("hostId" = Uuid, Path, description = "Host that should install and run the node.")),
    request_body = NodeCreate,
    responses(
        (status = 201, description = "Node installation was accepted and the node was created.", body = Node),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 422, response = ValidationFailed)
    )
)]
#[post("/hosts/{hostId}/nodes")]
pub async fn install_node_on_host(
    store: web::Data<Store>,
    host_id: web::Path<Uuid>,
    body: ValidatedJson<NodeCreate>,
) -> Result<HttpResponse, super::errors::ApiError> {
    let host_id = host_id.into_inner();
    let node_id = Uuid::new_v4();
    let node = store
        .create_node(host_id, node_id, body.into_inner().update)
        .await?;
    Ok(HttpResponse::Created().json(node))
}
