//! Extension writes persist desired state before asynchronous publication begins.
//! Responses expose flat configuration and pass through resource status unchanged.
//! Refresh changes the spec without waiting for publication to complete.
//! Removal is accepted before dependent-node cleanup; terminating resources remain
//! visible until finalizers complete, including when offline hosts defer cleanup.

use actix_web::{HttpResponse, delete, get, post, web};

use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::extension_source::ExtensionRegistration;
use crate::store::{ExtensionCreate, ResourceList, ResourceStore};
use suntitle_core::kubernetes::resources::{
    ExtensionStatus, SuntitleExtension, SuntitleExtensionSpec,
};

/// Public extension configuration and publication state without Kubernetes envelopes.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    /// Stable URL identity, independent of the display name.
    pub name: String,
    /// True while controller-owned dependent-node cleanup is pending.
    #[serde(default)]
    pub deleting: bool,
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
            deleting: resource.metadata.deletion_timestamp.is_some(),
            spec: resource.status.as_ref()
                .filter(|status| status.observed_generation == resource.metadata.generation)
                .and_then(|status| status.resolved_spec.clone())
                .unwrap_or(resource.spec),
            status: resource.status,
        }
    }
}

/// Registration accepts a source manifest or an explicit extension specification.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum ExtensionCreateRequest {
    /// Persist the source for asynchronous controller-owned manifest resolution.
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

/// Persist registration intent without fetching extension sources.
#[utoipa::path(post, path = "/extensions", tag = "Extensions", operation_id = "createExtension",
    summary = "Register an extension",
    description = "Register from a source manifest or an explicit specification. OCI source registration requires separate manifest YAML/JSON contents. The backend controller reads or parses the manifest asynchronously and reports resolution or preparation failures in status. Publication and host installation remain asynchronous.",
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
        ExtensionCreateRequest::Source(source) => source.into_create()?,
        ExtensionCreateRequest::Specification(specification) => specification,
    };
    let extension = store.create_extension(request).await?;
    let extension = Extension::from(extension);
    Ok(HttpResponse::Created()
        .insert_header(("Location", format!("/api/v1/extensions/{}", extension.name)))
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

/// Request removal; the controller retains terminating resources until dependent nodes finish cleanup.
#[utoipa::path(delete, path = "/extensions/{name}", tag = "Extensions", operation_id = "deleteExtension",
    description = "Request asynchronous removal of the extension and its dependent nodes. Terminating extensions remain visible until cleanup completes, including while a host is offline.",
    params(("name" = String, Path)),
    responses((status = 202, description = "Removal requested"), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[delete("/extensions/{name}")]
pub async fn delete_extension(
    store: web::Data<ResourceStore>,
    name: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    store.delete_extension(&name).await?;
    Ok(HttpResponse::Accepted().finish())
}
