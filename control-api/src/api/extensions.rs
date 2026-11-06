use actix_web::{HttpResponse, get, post, web};

use super::errors::{ApiError, ApiResult, ErrorEnvelope};
use crate::store::{ExtensionCreate, ResourceList, ResourceStore};
use suntitle_core::kubernetes::resources::SuntitleExtension;

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
    description = "Persist desired extension state. Success does not imply artifact publication or host installation.",
    request_body = ExtensionCreate,
    responses((status = 201, body = SuntitleExtension, headers(("Location" = String, description = "Created extension URL"))),
        (status = 400, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
#[post("/extensions")]
pub async fn create_extension(
    store: web::Data<ResourceStore>,
    body: web::Json<ExtensionCreate>,
) -> Result<HttpResponse, ApiError> {
    let extension = store.create_extension(body.into_inner()).await?;
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
