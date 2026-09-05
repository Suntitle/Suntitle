use actix_web::{get, post, put, web};
use uuid::Uuid;

use super::errors::{ApiResult, BadRequest, NotFound, Problem, Unauthorized, ValidationFailed};
use super::models::{ListNodesQuery, Node, NodeList, NodeUpdate};

#[utoipa::path(
    get,
    path = "/nodes",
    tag = "Nodes",
    operation_id = "listNodes",
    summary = "List connected nodes and their connections",
    description = "Returns nodes currently known by the backend. Each node includes its type, nested host, type-specific settings, and links to peer nodes.",
    params(ListNodesQuery),
    responses(
        (status = 200, description = "Nodes and their connection topology.", body = NodeList),
        (status = 401, response = Unauthorized)
    )
)]
#[get("/nodes")]
pub async fn list_nodes(_query: web::Query<ListNodesQuery>) -> ApiResult<NodeList> {
    todo!()
}

#[utoipa::path(
    get,
    path = "/nodes/{nodeId}",
    tag = "Nodes",
    operation_id = "getNode",
    summary = "Get a node",
    params(("nodeId" = Uuid, Path, description = "Unique node identifier.")),
    responses(
        (status = 200, description = "Node details and connections.", body = Node),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound)
    )
)]
#[get("/nodes/{nodeId}")]
pub async fn get_node(_node_id: web::Path<Uuid>) -> ApiResult<Node> {
    todo!()
}

#[utoipa::path(
    put,
    path = "/nodes/{nodeId}",
    tag = "Nodes",
    operation_id = "updateNode",
    summary = "Replace editable node information and settings",
    description = "Replaces operator-managed fields. The request `type` and `settings` must match the node's existing type. `inputNodeId` selects an upstream node whose type is allowed by the node type's `inputTypes`. ID, type, host, status, outgoing connection topology, and timestamps are server-managed.",
    params(("nodeId" = Uuid, Path, description = "Unique node identifier.")),
    request_body = NodeUpdate,
    responses(
        (status = 200, description = "Updated node.", body = Node),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "Request type conflicts with the existing node type.", body = Problem, content_type = "application/problem+json"),
        (status = 422, response = ValidationFailed)
    )
)]
#[put("/nodes/{nodeId}")]
pub async fn update_node(
    _node_id: web::Path<Uuid>,
    _body: web::Json<NodeUpdate>,
) -> ApiResult<Node> {
    todo!()
}

#[utoipa::path(
    post,
    path = "/nodes/{nodeId}/restart",
    tag = "Nodes",
    operation_id = "restartNode",
    summary = "Restart a node",
    description = "Restarts the selected node while preserving its configuration and connections.",
    params(("nodeId" = Uuid, Path, description = "Unique node identifier.")),
    responses(
        (status = 200, description = "Restarted node.", body = Node),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "The node cannot be restarted in its current state.", body = Problem, content_type = "application/problem+json")
    )
)]
#[post("/nodes/{nodeId}/restart")]
pub async fn restart_node(_node_id: web::Path<Uuid>) -> ApiResult<Node> {
    todo!()
}
