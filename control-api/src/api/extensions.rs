use actix_web::{HttpResponse, get, post, web};

use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::extension_source::ExtensionRegistration;
use crate::store::{ExtensionCreate, ResourceList, ResourceStore};
use suntitle_core::kubernetes::resources::SuntitleExtension;

/// Registration accepts a source manifest or an explicit extension specification.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum ExtensionCreateRequest {
    /// Resolve configuration from the selected source before persistence.
    Source(ExtensionRegistration),
    /// Preserve callers that already provide the complete specification.
    Specification(ExtensionCreate),
}

#[utoipa::path(get, path = "/extensions", tag = "Extensions", operation_id = "listExtensions",
    summary = "List registered extensions",
    responses((status = 200, body = ResourceList<SuntitleExtension>), (status = 503, body = ErrorEnvelope)))]
#[get("/extensions")]
pub async fn list_extensions(
    store: web::Data<ResourceStore>,
) -> ApiResult<ResourceList<SuntitleExtension>> {
    Ok(web::Json(store.list_extensions().await?))
}

#[utoipa::path(get, path = "/extensions/{name}", tag = "Extensions", operation_id = "getExtension",
    params(("name" = String, Path)),
    responses((status = 200, body = SuntitleExtension), (status = 404, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[get("/extensions/{name}")]
pub async fn get_extension(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> ApiResult<SuntitleExtension> {
    Ok(web::Json(store.extension(&name).await?))
}

#[utoipa::path(post, path = "/extensions", tag = "Extensions", operation_id = "createExtension",
    summary = "Register an extension",
    description = "Register from a source manifest or an explicit specification. Source registration reads manifest.yaml, manifest.yml, or manifest.json and overrides manifest source fields with the selected source. Publication and host installation remain asynchronous.",
    request_body = ExtensionCreateRequest,
    responses((status = 201, body = SuntitleExtension, headers(("Location" = String, description = "Created extension URL"))),
        (status = 400, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/extensions")]
pub async fn create_extension(
    store: web::Data<ResourceStore>,
    body: web::Json<ExtensionCreateRequest>,
) -> Result<HttpResponse, ApiError> {
    let request = match body.into_inner() {
        ExtensionCreateRequest::Source(source) => source.resolve().await?,
        ExtensionCreateRequest::Specification(specification) => specification,
    };
    let extension = store.create_extension(request).await?;
    use kube::ResourceExt;
    Ok(HttpResponse::Created()
        .insert_header((
            "Location",
            format!("/api/v1/extensions/{}", extension.name_any()),
        ))
        .json(extension))
}

#[utoipa::path(post, path = "/extensions/{name}/refresh", tag = "Extensions", operation_id = "refreshExtension",
    summary = "Request extension preparation from its source",
    description = "Persist a fresh refresh token. Preparation is asynchronous; pending requests may coalesce. Stored publication status may describe an older generation.",
    params(("name" = String, Path)),
    responses((status = 202, body = SuntitleExtension), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/extensions/{name}/refresh")]
pub async fn refresh_extension(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Accepted().json(store.refresh_extension(&name).await?))
}
