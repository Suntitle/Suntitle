//! Extension writes persist desired state before asynchronous publication begins.
//! Responses expose flat configuration and pass through resource status unchanged.
//! Refresh changes the spec without waiting for publication to complete.

use actix_web::{HttpResponse, get, post, web};

use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::extension_source::ExtensionRegistration;
use crate::store::{ExtensionCreate, ResourceList, ResourceStore};
use suntitle_core::kubernetes::resources::{ExtensionStatus, SuntitleExtension, SuntitleExtensionSpec};

/// Public extension configuration and publication state without Kubernetes envelopes.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    /// Stable URL identity, independent of the display name.
    pub name: String,
    /// Desired source and adapter fields exposed at the response's top level.
    #[serde(flatten)]
    pub spec: SuntitleExtensionSpec,
    /// Deferred publication progress or terminal outcome; never host installation state.
    pub status: Option<ExtensionStatus>,
}

impl From<SuntitleExtension> for Extension {
    /// Extract public identity and configuration; preserve resource status unchanged.
    fn from(resource: SuntitleExtension) -> Self {
        use kube::ResourceExt;
        Self {
            name: resource.name_any(),
            spec: resource.spec,
            status: resource.status,
        }
    }
}

/// Registration accepts a source manifest or an explicit extension specification.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum ExtensionCreateRequest {
    /// Resolve configuration from the selected source before persistence.
    Source(ExtensionRegistration),
    /// Accept complete configuration directly as flat request fields.
    Specification(ExtensionCreate),
}

/// List flat extension records while preserving the collection revision.
#[utoipa::path(get, path = "/extensions", tag = "Extensions", operation_id = "listExtensions",
    summary = "List registered extensions",
    responses((status = 200, body = ResourceList<Extension>), (status = 503, body = ErrorEnvelope)))]
#[get("/extensions")]
pub async fn list_extensions(
    store: web::Data<ResourceStore>,
) -> ApiResult<ResourceList<Extension>> {
    let list = store.list_extensions().await?;
    Ok(web::Json(ResourceList {
        items: list.items.into_iter().map(Extension::from).collect(),
        resource_version: list.resource_version,
    }))
}

/// Read one extension by its stable URL identity.
#[utoipa::path(get, path = "/extensions/{name}", tag = "Extensions", operation_id = "getExtension",
    params(("name" = String, Path)),
    responses((status = 200, body = Extension), (status = 404, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[get("/extensions/{name}")]
pub async fn get_extension(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> ApiResult<Extension> {
    Ok(web::Json(store.extension(&name).await?.into()))
}

/// Persist resolved configuration and return its flat representation.
#[utoipa::path(post, path = "/extensions", tag = "Extensions", operation_id = "createExtension",
    summary = "Register an extension",
    description = "Register from a source manifest or an explicit specification. Source registration reads manifest.yaml, manifest.yml, or manifest.json and overrides manifest source fields with the selected source. Publication and host installation remain asynchronous.",
    request_body = ExtensionCreateRequest,
    responses((status = 201, body = Extension, headers(("Location" = String, description = "Created extension URL"))),
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
    let extension = Extension::from(extension);
    Ok(HttpResponse::Created()
        .insert_header((
            "Location",
            format!("/api/v1/extensions/{}", extension.name),
        ))
        .json(extension))
}

/// Request publication asynchronously and return its current public state.
#[utoipa::path(post, path = "/extensions/{name}/refresh", tag = "Extensions", operation_id = "refreshExtension",
    summary = "Request extension preparation from its source",
    description = "Persist a fresh refresh token. Preparation is asynchronous; pending requests may coalesce. Stored publication status may describe an earlier configuration.",
    params(("name" = String, Path)),
    responses((status = 202, body = Extension), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/extensions/{name}/refresh")]
pub async fn refresh_extension(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Accepted().json(Extension::from(store.refresh_extension(&name).await?)))
}
