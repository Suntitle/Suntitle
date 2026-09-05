use actix_web::{HttpResponse, get, post, web};
use uuid::Uuid;

use crate::registration::RegistrationServerMessage;
use crate::{DesiredNode, HostControlRegistry, HostRegistry, NodeRegistry};

use super::errors::{ApiResult, BadRequest, NotFound, Unauthorized, ValidationFailed};
use super::models::{HostList, ListHostsQuery, Node, NodeCreate, NodeInputUpdate, NodeUpdate};

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
    hosts: web::Data<HostRegistry>,
    query: web::Query<ListHostsQuery>,
) -> ApiResult<HostList> {
    let hosts = hosts.read().await;
    let mut items: Vec<_> = hosts
        .values()
        .filter(|host| query.status.is_none_or(|status| host.status == status))
        .cloned()
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
    hosts: web::Data<HostRegistry>,
    nodes: web::Data<NodeRegistry>,
    controls: web::Data<HostControlRegistry>,
    host_id: web::Path<Uuid>,
    body: web::Json<NodeCreate>,
) -> Result<HttpResponse, super::errors::ApiError> {
    let host_id = host_id.into_inner();
    let host = hosts
        .read()
        .await
        .get(&host_id)
        .cloned()
        .ok_or_else(|| super::errors::ApiError::not_found("host does not exist"))?;
    let update = create_to_update(body.into_inner());
    let node_id = Uuid::new_v4();
    let node = Node::from_update(node_id, host, &update);
    nodes.write().await.insert(
        node_id,
        DesiredNode {
            host_id,
            revision: 1,
            update: update.clone(),
            node: node.clone(),
        },
    );
    super::nodes::send_control(
        &controls,
        host_id,
        RegistrationServerMessage::ApplyNode {
            node_id,
            revision: 1,
            update,
        },
    )
    .await;
    Ok(HttpResponse::Created().json(node))
}

fn create_to_update(create: NodeCreate) -> NodeUpdate {
    match create {
        NodeCreate::AudioSource(value) => {
            NodeUpdate::AudioSource(super::models::AudioSourceNodeUpdate {
                base: value.base,
                variant: value.variant,
                input: NodeInputUpdate {
                    input_node_id: None,
                },
            })
        }
        NodeCreate::Transcriber(value) => {
            NodeUpdate::Transcriber(super::models::TranscriberNodeUpdate {
                base: value.base,
                variant: value.variant,
                input: NodeInputUpdate {
                    input_node_id: None,
                },
            })
        }
        NodeCreate::Translator(value) => {
            NodeUpdate::Translator(super::models::TranslatorNodeUpdate {
                base: value.base,
                variant: value.variant,
                input: NodeInputUpdate {
                    input_node_id: None,
                },
            })
        }
        NodeCreate::Display(value) => NodeUpdate::Display(super::models::DisplayNodeUpdate {
            base: value.base,
            variant: value.variant,
            input: NodeInputUpdate {
                input_node_id: None,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use actix_web::{App, test, web};
    use chrono::Utc;

    use super::*;
    use crate::api::models::{Host, HostStatus};

    fn host(id: &str, status: HostStatus) -> Host {
        Host {
            id: Uuid::parse_str(id).unwrap(),
            name: id.to_owned(),
            ip: "192.0.2.10".parse::<IpAddr>().unwrap(),
            version: "0.1.0".to_owned(),
            status,
            last_seen_at: Utc::now(),
            metadata: None,
        }
    }

    #[actix_web::test]
    async fn lists_registry_hosts_and_filters_by_status() {
        let registry = HostRegistry::default();
        {
            let mut hosts = registry.write().await;
            let online = host("20000000-0000-4000-8000-000000000002", HostStatus::Online);
            let offline = host("20000000-0000-4000-8000-000000000001", HostStatus::Offline);
            hosts.insert(online.id, online);
            hosts.insert(offline.id, offline);
        }
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(registry))
                .service(list_hosts),
        )
        .await;

        let all: HostList = test::call_and_read_body_json(
            &app,
            test::TestRequest::get().uri("/hosts").to_request(),
        )
        .await;
        assert_eq!(all.total, 2);
        assert_eq!(
            all.items[0].id,
            Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap()
        );

        let online: HostList = test::call_and_read_body_json(
            &app,
            test::TestRequest::get()
                .uri("/hosts?status=online")
                .to_request(),
        )
        .await;
        assert_eq!(online.total, 1);
        assert!(matches!(online.items[0].status, HostStatus::Online));
    }
}
