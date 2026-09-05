use std::fmt;

use actix_web::http::{StatusCode, header};
use actix_web::{HttpResponse, ResponseError};
use serde::{Deserialize, Serialize};
use utoipa::{ToResponse, ToSchema};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    #[schema(format = "uri-reference", default = "about:blank")]
    pub r#type: String,
    pub title: String,
    #[schema(minimum = 400, maximum = 599)]
    pub status: u16,
    #[schema(nullable = false)]
    pub detail: Option<String>,
    #[schema(format = "uri-reference", nullable = false)]
    pub instance: Option<String>,
    #[schema(nullable = false)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationError {
    #[schema(examples("settings.language"))]
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ValidationProblem {
    #[serde(flatten)]
    pub problem: Problem,
    #[schema(inline)]
    pub errors: Vec<ValidationError>,
}

/// The request is malformed.
#[derive(ToResponse)]
#[response(content_type = "application/problem+json")]
pub struct BadRequest(pub Problem);

/// A valid bearer token is required.
#[derive(ToResponse)]
#[response(
    content_type = "application/problem+json",
    headers(("WWW-Authenticate" = String))
)]
pub struct Unauthorized(pub Problem);

/// The requested resource was not found.
#[derive(ToResponse)]
#[response(content_type = "application/problem+json")]
pub struct NotFound(pub Problem);

/// One or more fields failed validation.
#[derive(ToResponse)]
#[response(content_type = "application/problem+json")]
pub struct ValidationFailed(pub ValidationProblem);

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    problem: Box<Problem>,
    authenticate: bool,
}

impl ApiError {
    pub fn unauthorized(instance: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            problem: Box::new(Problem {
                r#type: "about:blank".to_owned(),
                title: "Unauthorized".to_owned(),
                status: StatusCode::UNAUTHORIZED.as_u16(),
                detail: Some("A valid bearer token is required.".to_owned()),
                instance: Some(instance.into()),
                request_id: None,
            }),
            authenticate: true,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.problem.title)
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        self.status
    }

    fn error_response(&self) -> HttpResponse {
        let mut response = HttpResponse::build(self.status);
        response.content_type("application/problem+json");
        if self.authenticate {
            response.insert_header((header::WWW_AUTHENTICATE, "Bearer"));
        }
        response.json(&*self.problem)
    }
}

pub type ApiResult<T> = Result<actix_web::web::Json<T>, ApiError>;
