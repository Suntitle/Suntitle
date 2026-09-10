pub mod auth;
pub mod errors;
pub mod hosts;
pub mod models;
pub mod node_types;
pub mod nodes;
pub mod validated_json;

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::kubernetes::store::ResourceStore;

use self::errors::{BadRequest, NotFound, Unauthorized, ValidationFailed};
use self::models::*;

const DESCRIPTION: &str = "Resource-oriented control API. Successful writes persist desired state; runtime application is asynchronous. Kubernetes schema/CEL validate specs; API-server errors preserve their HTTP status, reason, message, and details.";

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
        nodes::restart_node,
        nodes::delete_node
    ),
    components(
        schemas(
            LoginRequest, LoginResponse, User,
            NodeType, NodeSettingValue, NodeSettingOption, NodeSettingDefinition,
            NodeTypeDefinition, NodeTypeDefinitionList,
            AudioSourceSettings, TranscriberSettings, TranslatorSettings, DisplaySettings,
            LanguageTag, errors::ErrorEnvelope,
            crate::kubernetes::resources::SuntitleHost,
            crate::kubernetes::resources::SuntitleNode,
            crate::kubernetes::store::HostList,
            crate::kubernetes::store::NodeList,
            crate::kubernetes::store::NodeCreate,
            crate::kubernetes::store::NodeUpdate
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
        for (path, item) in &mut openapi.paths.paths {
            if path == "/auth/login" {
                continue;
            }
            for operation in [
                &mut item.get,
                &mut item.post,
                &mut item.put,
                &mut item.delete,
            ]
            .into_iter()
            .flatten()
            {
                for (status, description) in [("401", "Unauthorized"), ("403", "Forbidden")] {
                    operation
                        .responses
                        .responses
                        .entry(status.to_owned())
                        .or_insert_with(|| {
                            utoipa::openapi::ResponseBuilder::new()
                                .description(description)
                                .content(
                                    "application/json",
                                    utoipa::openapi::Content::new(Some(
                                        utoipa::openapi::Ref::from_schema_name("ErrorEnvelope"),
                                    )),
                                )
                                .build()
                                .into()
                        });
                }
            }
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
    store: ResourceStore,
) {
    config.app_data(actix_web::web::Data::new(auth::AuthState::default()));
    config.app_data(actix_web::web::Data::new(store));
    config.app_data(actix_web::web::JsonConfig::default().error_handler(|_, _| {
        errors::ApiError::problem(
            actix_web::http::StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "Malformed JSON request body.",
        )
        .into()
    }));
    config.route(
        "/readyz",
        actix_web::web::get().to(|store: actix_web::web::Data<ResourceStore>| async move {
            store.ready().await?;
            Ok::<_, errors::ApiError>(actix_web::HttpResponse::Ok().finish())
        }),
    );
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
            .service(nodes::restart_node)
            .service(nodes::delete_node),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_contract_matches_checked_in_document() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let committed: serde_yaml::Value =
            serde_yaml::from_str(include_str!("../../../specs/openapi.yaml")).unwrap();
        assert_eq!(serde_json::to_value(committed).unwrap(), document);
        assert!(
            document["components"]["schemas"]["SuntitleNode"]["properties"]["metadata"].is_object()
        );
        assert!(document["paths"]["/nodes/{name}/restart"]["post"]["responses"]["202"].is_object());
    }
}
