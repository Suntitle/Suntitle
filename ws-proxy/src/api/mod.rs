pub mod auth;
pub mod errors;
pub mod hosts;
pub mod models;
pub mod node_types;
pub mod nodes;

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::{ControlPlaneState, HostRegistry};

use self::errors::{BadRequest, NotFound, Unauthorized, ValidationFailed};
use self::models::*;

const DESCRIPTION: &str = "API for authenticating operators and managing hosts and nodes in the live\nsubtitle pipeline. Connection topology is represented by the `connections`\nfield of each node; there is intentionally no separate routes resource.\n";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Suntitle Control API",
        version = "0.1.0",
        description = DESCRIPTION
    ),
    paths(
        auth::login,
        auth::logout,
        auth::get_current_user,
        hosts::list_hosts,
        hosts::install_node_on_host,
        node_types::list_node_types,
        nodes::list_nodes,
        nodes::get_node,
        nodes::update_node,
        nodes::restart_node
    ),
    components(
        schemas(
            LoginRequest, LoginResponse, User,
            NodeType, NodeSettingValue, NodeSettingOption, NodeSettingDefinition,
            NodeTypeDefinition, NodeTypeDefinitionList,
            NodeStatus, HostStatus, Host, HostList, ConnectionStatus, DataType,
            NodeBase, NodeConnection, Node,
            AudioSourceNode, TranscriberNode, TranslatorNode, DisplayNode,
            AudioSourceVariant, TranscriberVariant, TranslatorVariant, DisplayVariant,
            NodeUpdateBase, NodeInputUpdate, NodeUpdate, NodeCreate,
            AudioSourceNodeCreate, TranscriberNodeCreate, TranslatorNodeCreate, DisplayNodeCreate,
            AudioSourceNodeUpdate, TranscriberNodeUpdate, TranslatorNodeUpdate, DisplayNodeUpdate,
            AudioSourceSettings, TranscriberSettings, TranslatorSettings, DisplaySettings,
            LanguageTag, NodeList,
            errors::Problem, errors::ValidationProblem
        ),
        responses(BadRequest, Unauthorized, NotFound, ValidationFailed)
    ),
    modifiers(&SecurityAddon),
    security(("bearerAuth" = [])),
    tags(
        (name = "Authentication"),
        (name = "Hosts"),
        (name = "Node types"),
        (name = "Nodes")
    ),
    servers((url = "/api/v1", description = "API version 1"))
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi.info.license = None;
        if let Some(login) = openapi.paths.paths.get_mut("/auth/login")
            && let Some(operation) = login.post.as_mut()
        {
            operation.security = Some(Vec::new());
        }
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearerAuth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("opaque")
                        .build(),
                ),
            );
        }
    }
}

/// Register REST routes and their shared in-memory authentication state.
pub fn configure(config: &mut utoipa_actix_web::service_config::ServiceConfig) {
    configure_with_state(config, ControlPlaneState::default());
}

/// Register REST routes backed by the supplied host registry.
pub fn configure_with_hosts(
    config: &mut utoipa_actix_web::service_config::ServiceConfig,
    hosts: HostRegistry,
) {
    let state = ControlPlaneState {
        hosts,
        ..ControlPlaneState::default()
    };
    configure_with_state(config, state);
}

pub fn configure_with_state(
    config: &mut utoipa_actix_web::service_config::ServiceConfig,
    state: ControlPlaneState,
) {
    config.app_data(actix_web::web::Data::new(auth::AuthState::default()));
    config.app_data(actix_web::web::Data::new(state.hosts));
    config.app_data(actix_web::web::Data::new(state.nodes));
    config.app_data(actix_web::web::Data::new(state.host_controls));
    config.service(
        utoipa_actix_web::scope::scope("/api/v1")
            .service(auth::login)
            .service(auth::logout)
            .service(auth::get_current_user)
            .service(hosts::list_hosts)
            .service(hosts::install_node_on_host)
            .service(node_types::list_node_types)
            .service(nodes::list_nodes)
            .service(nodes::get_node)
            .service(nodes::update_node)
            .service(nodes::restart_node),
    );
}

#[cfg(test)]
mod tests {
    use actix_web::App;
    use utoipa_actix_web::AppExt;

    use super::*;

    #[test]
    fn generated_contract_contains_all_routes_and_named_schemas() {
        let document = ApiDoc::openapi();
        assert_eq!(document.paths.paths.len(), 9);

        let components = document.components.expect("OpenAPI components");
        for schema in [
            "Node",
            "NodeCreate",
            "NodeUpdate",
            "AudioSourceSettings",
            "TranscriberSettings",
            "TranslatorSettings",
            "DisplaySettings",
            "Problem",
            "ValidationProblem",
        ] {
            assert!(components.schemas.contains_key(schema), "missing {schema}");
        }
        assert!(components.security_schemes.contains_key("bearerAuth"));
    }

    #[actix_web::test]
    async fn rest_scope_registers_with_actix() {
        let app = App::new().into_utoipa_app().configure(configure).into_app();
        actix_web::test::init_service(app).await;
    }
}
