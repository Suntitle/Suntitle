use std::future::Future;
use std::pin::Pin;

use actix_web::{Error, FromRequest, HttpRequest, dev::Payload, web};
use serde::de::DeserializeOwned;
use validator::Validate;

use super::errors::ApiError;

/// JSON request body validated before the handler runs.
///
/// Uses Actix's JSON extractor so configured payload limits and JSON error
/// handling still apply. Struct validation failures use the API's 422 response.
pub struct ValidatedJson<T>(T);

impl<T> ValidatedJson<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: DeserializeOwned + Validate + 'static> FromRequest for ValidatedJson<T> {
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let body = web::Json::<T>::from_request(req, payload);
        Box::pin(async move {
            let body = body.await?.into_inner();
            body.validate().map_err(ApiError::validation)?;
            Ok(Self(body))
        })
    }
}

#[cfg(test)]
mod tests {
    use actix_web::{App, HttpResponse, http::StatusCode, test};

    use super::*;
    use crate::api::models::NodeCreate;

    #[actix_web::test]
    async fn preserves_json_errors_and_configured_payload_limits() {
        let app = test::init_service(
            App::new()
                .app_data(web::JsonConfig::default().limit(128))
                .route(
                    "/",
                    web::post().to(|body: ValidatedJson<NodeCreate>| async {
                        HttpResponse::Ok().json(body.into_inner())
                    }),
                ),
        )
        .await;
        for (payload, expected) in [
            ("{".to_owned(), StatusCode::BAD_REQUEST),
            (" ".repeat(129), StatusCode::PAYLOAD_TOO_LARGE),
        ] {
            let response = test::call_service(
                &app,
                test::TestRequest::post()
                    .uri("/")
                    .insert_header(("content-type", "application/json"))
                    .set_payload(payload)
                    .to_request(),
            )
            .await;
            assert_eq!(response.status(), expected);
        }
    }
}
