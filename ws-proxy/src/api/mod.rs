pub mod auth;
pub mod errors;
pub mod hosts;
pub mod models;
pub mod node_types;
pub mod nodes;
pub mod validated_json;

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::storage::Store;

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
            NodeSettings,
            NodeUpdateBase, NodeInputUpdate, NodeUpdate, NodeCreate,
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

pub fn configure_with_store(
    config: &mut utoipa_actix_web::service_config::ServiceConfig,
    store: Store,
) {
    config.app_data(actix_web::web::Data::new(auth::AuthState::default()));
    config.app_data(actix_web::web::Data::new(store));
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
            "AudioSourceNode",
            "TranscriberNode",
            "TranslatorNode",
            "DisplayNode",
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
        let app = App::new()
            .into_utoipa_app()
            .configure(|c| {
                configure_with_store(
                    c,
                    std::sync::Arc::new(crate::storage::MemoryStore::default()),
                )
            })
            .into_app();
        actix_web::test::init_service(app).await;
    }

    #[test]
    fn node_contract_has_four_named_variants() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let schemas = &document["components"]["schemas"];
        let variants = schemas["Node"]["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 4);
        for (variant, (tag, node, settings)) in variants.iter().zip([
            ("audio_source", "AudioSourceNode", "AudioSourceSettings"),
            ("transcriber", "TranscriberNode", "TranscriberSettings"),
            ("translator", "TranslatorNode", "TranslatorSettings"),
            ("display", "DisplayNode", "DisplaySettings"),
        ]) {
            let parts = variant["allOf"].as_array().unwrap();
            assert!(
                parts
                    .iter()
                    .any(|part| part["$ref"] == format!("#/components/schemas/{node}"))
            );
            assert!(parts.iter().any(|part| {
                part["properties"]["type"]["enum"] == serde_json::json!([tag])
                    && part["required"] == serde_json::json!(["type"])
            }));
            let fields = schemas[node]["allOf"].as_array().unwrap();
            assert!(
                fields
                    .iter()
                    .any(|part| part["$ref"] == "#/components/schemas/NodeBase")
            );
            assert!(fields.iter().any(|part| {
                part["properties"]["settings"]["$ref"] == format!("#/components/schemas/{settings}")
                    && part["required"] == serde_json::json!(["settings"])
            }));
        }
    }

    #[test]
    fn node_contract_composes_common_fields_with_tagged_settings() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let schemas = &document["components"]["schemas"];
        let create = schemas["NodeCreate"]["allOf"].as_array().unwrap();
        assert_eq!(create.len(), 1);
        assert_eq!(create[0]["$ref"], "#/components/schemas/NodeUpdate");
        let parts = schemas["NodeUpdate"]["allOf"].as_array().unwrap();
        assert!(
            parts
                .iter()
                .any(|part| part["$ref"] == "#/components/schemas/NodeSettings")
        );
        let variants = schemas["NodeSettings"]["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 4);
        for (variant, (tag, settings)) in variants.iter().zip([
            ("audio_source", "AudioSourceSettings"),
            ("transcriber", "TranscriberSettings"),
            ("translator", "TranslatorSettings"),
            ("display", "DisplaySettings"),
        ]) {
            assert_eq!(
                variant["properties"]["type"]["enum"],
                serde_json::json!([tag])
            );
            assert_eq!(
                variant["properties"]["settings"]["$ref"],
                format!("#/components/schemas/{settings}")
            );
            let required = variant["required"].as_array().unwrap();
            assert!(required.contains(&serde_json::json!("type")));
            assert!(required.contains(&serde_json::json!("settings")));
        }
        let committed: serde_yaml::Value =
            serde_yaml::from_str(include_str!("../../../specs/openapi.yaml")).unwrap();
        assert_eq!(serde_json::to_value(committed).unwrap(), document);
    }
}
