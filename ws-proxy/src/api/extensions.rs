use actix_web::{HttpResponse, get, post, web};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::errors::{ApiError, BadRequest, ErrorEnvelope, NotFound, ValidationFailed};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Extension {
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub source_type: ExtensionSourceType,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionCreate {
    pub source: String,
    pub source_type: ExtensionSourceType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionSourceType {
    Local,
    Git,
    Zip,
}

#[utoipa::path(
    get,
    path = "/extensions",
    tag = "Extensions",
    operation_id = "listExtensions",
    summary = "List installed extensions",
    responses(
        (status = 200, description = "Installed extensions.", body = Vec<Extension>),
        (status = 503, description = "Extension storage is unavailable.", body = ErrorEnvelope)
    )
)]
#[get("/extensions")]
pub async fn list_extensions() -> web::Json<Vec<Extension>> {
    todo!()
}

#[utoipa::path(
    post,
    path = "/extensions",
    tag = "Extensions",
    operation_id = "createExtension",
    summary = "Install an extension",
    request_body = ExtensionCreate,
    responses(
        (status = 201, description = "Extension installed.", body = Extension, headers(("Location" = String, description = "Created extension URL"))),
        (status = 400, response = BadRequest),
        (status = 409, description = "The extension is already installed.", body = ErrorEnvelope),
        (status = 422, response = ValidationFailed),
        (status = 503, description = "Extension storage is unavailable.", body = ErrorEnvelope)
    )
)]
#[post("/extensions")]
pub async fn create_extension(_body: web::Json<ExtensionCreate>) -> Result<HttpResponse, ApiError> {
    todo!()
}

#[utoipa::path(
    post,
    path = "/extensions/{name}/refresh",
    tag = "Extensions",
    operation_id = "refreshExtension",
    summary = "Refresh an extension from its source",
    params(("name" = String, Path)),
    responses(
        (status = 202, description = "Extension refresh accepted.", body = Extension),
        (status = 404, response = NotFound),
        (status = 409, description = "The extension cannot be refreshed in its current state.", body = ErrorEnvelope),
        (status = 503, description = "Extension source is unavailable.", body = ErrorEnvelope)
    )
)]
#[post("/extensions/{name}/refresh")]
pub async fn refresh_extension(_name: web::Path<String>) -> Result<HttpResponse, ApiError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_responses_match_the_extension_contract() {
        serde_json::from_str::<Vec<Extension>>(include_str!(
            "../../../mockserver/api/v1/extensions.json"
        ))
        .expect("list fixture should contain extensions");
        serde_json::from_str::<Extension>(include_str!(
            "../../../mockserver/api/v1/extensions.post.201.json"
        ))
        .expect("create fixture should contain an extension");
        serde_json::from_str::<Extension>(include_str!(
            "../../../mockserver/api/v1/extensions/obs-captions/refresh.post.202.json"
        ))
        .expect("refresh fixture should contain an extension");
    }
}
