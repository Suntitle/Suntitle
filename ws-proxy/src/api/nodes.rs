use super::validated_json::ValidatedJson;
use actix_web::{get, post, put, web};
use uuid::Uuid;

use super::errors::{ApiResult, BadRequest, NotFound, Problem, Unauthorized, ValidationFailed};
use super::models::{ListNodesQuery, Node, NodeList, NodeType, NodeUpdate};
use crate::DesiredNode;
use crate::storage::{Mutation, Store};

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
pub async fn list_nodes(
    store: web::Data<Store>,
    query: web::Query<ListNodesQuery>,
) -> ApiResult<NodeList> {
    let mut items: Vec<_> = store
        .nodes()
        .await?
        .into_iter()
        .filter(|node| {
            query
                .node_type
                .is_none_or(|expected| node.node_type() == expected)
                && query
                    .status
                    .is_none_or(|expected| node.status() == expected)
        })
        .collect();
    items.sort_unstable_by_key(Node::id);
    Ok(web::Json(NodeList {
        total: items.len() as u64,
        items,
    }))
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
pub async fn get_node(store: web::Data<Store>, node_id: web::Path<Uuid>) -> ApiResult<Node> {
    Ok(web::Json(store.node(node_id.into_inner()).await?))
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
    store: web::Data<Store>,
    node_id: web::Path<Uuid>,
    body: ValidatedJson<NodeUpdate>,
) -> ApiResult<Node> {
    let node_id = node_id.into_inner();
    let update = body.into_inner();
    let response = store.apply_node_change(node_id, update).await?;
    Ok(web::Json(response))
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
pub async fn restart_node(store: web::Data<Store>, node_id: web::Path<Uuid>) -> ApiResult<Node> {
    let node_id = node_id.into_inner();
    let state = store.mutate(Mutation::Restart(node_id)).await?;
    let response = state.nodes[&node_id].node.clone();
    Ok(web::Json(response))
}

pub(crate) fn validate_input(
    node_id: Uuid,
    update: &NodeUpdate,
    registry: &std::collections::HashMap<Uuid, DesiredNode>,
) -> Result<(), super::errors::ApiError> {
    let input_id = update.input_node_id();
    let Some(input_id) = input_id else {
        return Ok(());
    };
    if input_id == node_id {
        return Err(super::errors::ApiError::problem(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            "Validation Failed",
            "a node cannot use itself as input",
        ));
    }
    let input = registry.get(&input_id).ok_or_else(|| {
        super::errors::ApiError::problem(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            "Validation Failed",
            "inputNodeId does not identify an existing node",
        )
    })?;
    let allowed = match update.node_type() {
        NodeType::AudioSource => false,
        NodeType::Transcriber => input.node.node_type() == NodeType::AudioSource,
        NodeType::Translator => input.node.node_type() == NodeType::Transcriber,
        NodeType::Display => matches!(
            input.node.node_type(),
            NodeType::Transcriber | NodeType::Translator
        ),
    };
    if !allowed {
        return Err(super::errors::ApiError::problem(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            "Validation Failed",
            "input node type is incompatible with this node type",
        ));
    }
    Ok(())
}
