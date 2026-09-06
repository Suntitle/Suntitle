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
    errors: Vec<ValidationError>,
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
            errors: Vec::new(),
        }
    }

    pub fn problem(
        status: StatusCode,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            status,
            problem: Box::new(Problem {
                r#type: "about:blank".to_owned(),
                title: title.into(),
                status: status.as_u16(),
                detail: Some(detail.into()),
                instance: None,
                request_id: None,
            }),
            authenticate: false,
            errors: Vec::new(),
        }
    }

    pub fn validation(errors: validator::ValidationErrors) -> Self {
        fn flatten(
            prefix: &str,
            errors: &validator::ValidationErrors,
            out: &mut Vec<ValidationError>,
        ) {
            for (field, kind) in errors.errors() {
                let field = match field.as_ref() {
                    "sample_rate_hz" => "sampleRateHz",
                    "device_name" => "deviceName",
                    "beam_size" => "beamSize",
                    "source_language" => "sourceLanguage",
                    "target_language" => "targetLanguage",
                    "max_lines" => "maxLines",
                    "font_scale" => "fontScale",
                    other => other,
                };
                let path = if field == "base" || field == "update" {
                    prefix.to_owned()
                } else if prefix.is_empty() {
                    field.to_owned()
                } else {
                    format!("{prefix}.{field}")
                };
                match kind {
                    validator::ValidationErrorsKind::Field(errors) => {
                        out.extend(errors.iter().map(|error| ValidationError {
                            path: path.clone(),
                            message: error.message.as_deref().map(str::to_owned).unwrap_or_else(
                                || format!("{} validation failed: {:?}", error.code, error.params),
                            ),
                        }))
                    }
                    validator::ValidationErrorsKind::Struct(errors) => flatten(&path, errors, out),
                    validator::ValidationErrorsKind::List(errors) => {
                        for (index, errors) in errors {
                            flatten(&format!("{path}[{index}]"), errors, out);
                        }
                    }
                }
            }
        }
        let mut result = Self::problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Validation Failed",
            "One or more fields failed validation.",
        );
        flatten("", &errors, &mut result.errors);
        result.errors.sort_by(|a, b| a.path.cmp(&b.path));
        result
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::problem(StatusCode::NOT_FOUND, "Not Found", detail)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::problem(StatusCode::CONFLICT, "Conflict", detail)
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
        if self.errors.is_empty() {
            response.json(&*self.problem)
        } else {
            response.json(ValidationProblem {
                problem: (*self.problem).clone(),
                errors: self.errors.clone(),
            })
        }
    }
}

pub type ApiResult<T> = Result<actix_web::web::Json<T>, ApiError>;
