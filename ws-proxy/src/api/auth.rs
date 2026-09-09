use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, get, post, web};
use uuid::Uuid;

use super::errors::{ApiError, ApiResult, BadRequest, ErrorEnvelope, Unauthorized};
use super::models::{LoginRequest, LoginResponse, User};

const TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Default)]
pub struct AuthState {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

#[derive(Clone)]
struct Session {
    user: User,
    expires_at: Instant,
}

impl AuthState {
    fn create_session(&self, username: String) -> (String, Session) {
        let token = Uuid::new_v4().simple().to_string();
        let session = Session {
            user: User {
                id: Uuid::new_v4(),
                username,
            },
            expires_at: Instant::now() + TOKEN_LIFETIME,
        };
        let mut sessions = self
            .sessions
            .write()
            .expect("authentication state lock poisoned");
        sessions.retain(|_, existing| existing.expires_at > Instant::now());
        sessions.insert(token.clone(), session.clone());
        (token, session)
    }

    fn session(&self, token: &str) -> Option<Session> {
        let mut sessions = self
            .sessions
            .write()
            .expect("authentication state lock poisoned");
        let session = sessions.get(token).cloned()?;
        if session.expires_at <= Instant::now() {
            sessions.remove(token);
            return None;
        }
        Some(session)
    }

    fn revoke(&self, token: &str) -> bool {
        self.sessions
            .write()
            .expect("authentication state lock poisoned")
            .remove(token)
            .is_some()
    }
}

fn bearer_token(request: &HttpRequest) -> Result<&str, ApiError> {
    let unauthorized = || ApiError::unauthorized(request.path());
    let value = request
        .headers()
        .get(header::AUTHORIZATION)
        .ok_or_else(unauthorized)?
        .to_str()
        .map_err(|_| unauthorized())?;
    let (scheme, token) = value.split_once(' ').ok_or_else(unauthorized)?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.contains(char::is_whitespace)
    {
        return Err(unauthorized());
    }
    Ok(token)
}

#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "Authentication",
    operation_id = "login",
    summary = "Log in",
    description = "Exchange user credentials for a bearer access token.",
    security(()),
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful.", body = LoginResponse),
        (status = 400, response = BadRequest),
        (status = 401, description = "Invalid username or password.", body = ErrorEnvelope, content_type = "application/json")
    )
)]
#[post("/auth/login")]
pub async fn login(
    state: web::Data<AuthState>,
    body: web::Json<LoginRequest>,
) -> ApiResult<LoginResponse> {
    let (access_token, session) = state.create_session(body.into_inner().username);
    Ok(web::Json(LoginResponse {
        access_token,
        token_type: "Bearer".to_owned(),
        expires_in: TOKEN_LIFETIME.as_secs(),
        user: session.user,
    }))
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "Authentication",
    operation_id = "logout",
    summary = "Log out",
    description = "Revoke the bearer token used for this request.",
    responses(
        (status = 204, description = "Logout successful."),
        (status = 401, response = Unauthorized)
    )
)]
#[post("/auth/logout")]
pub async fn logout(
    request: HttpRequest,
    state: web::Data<AuthState>,
) -> Result<HttpResponse, ApiError> {
    let token = bearer_token(&request)?;
    if !state.revoke(token) {
        return Err(ApiError::unauthorized(request.path()));
    }
    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "Authentication",
    operation_id = "getCurrentUser",
    summary = "Get the authenticated user",
    description = "Validates the bearer token and returns its user. A missing, expired, revoked, or otherwise invalid token returns `401 Unauthorized`.",
    responses(
        (status = 200, description = "The bearer token is valid.", body = User),
        (status = 401, response = Unauthorized)
    )
)]
#[get("/auth/me")]
pub async fn get_current_user(
    request: HttpRequest,
    state: web::Data<AuthState>,
) -> ApiResult<User> {
    let token = bearer_token(&request)?;
    let session = state
        .session(token)
        .ok_or_else(|| ApiError::unauthorized(request.path()))?;
    Ok(web::Json(session.user))
}

#[cfg(test)]
mod tests {
    use actix_web::http::{StatusCode, header};
    use actix_web::{App, test, web};

    use super::*;

    fn app_state() -> web::Data<AuthState> {
        web::Data::new(AuthState::default())
    }

    #[actix_web::test]
    async fn login_accepts_arbitrary_credentials_and_creates_distinct_tokens() {
        let state = app_state();
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(login)
                .service(get_current_user),
        )
        .await;

        let first: LoginResponse = test::call_and_read_body_json(
            &app,
            test::TestRequest::post()
                .uri("/auth/login")
                .set_json(&LoginRequest {
                    username: "alice".to_owned(),
                    password: "anything".to_owned(),
                })
                .to_request(),
        )
        .await;
        let second: LoginResponse = test::call_and_read_body_json(
            &app,
            test::TestRequest::post()
                .uri("/auth/login")
                .set_json(&LoginRequest {
                    username: "alice".to_owned(),
                    password: "different".to_owned(),
                })
                .to_request(),
        )
        .await;

        assert_ne!(first.access_token, second.access_token);
        assert_eq!(first.token_type, "Bearer");
        assert_eq!(first.expires_in, 86_400);
        assert_eq!(first.user.username, "alice");

        let user: User = test::call_and_read_body_json(
            &app,
            test::TestRequest::get()
                .uri("/auth/me")
                .insert_header((
                    header::AUTHORIZATION,
                    format!("Bearer {}", first.access_token),
                ))
                .to_request(),
        )
        .await;
        assert_eq!(user.id, first.user.id);
        assert_eq!(user.username, first.user.username);
    }

    #[actix_web::test]
    async fn logout_revokes_the_current_token() {
        let state = app_state();
        let (token, _) = state.create_session("operator".to_owned());
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(logout)
                .service(get_current_user),
        )
        .await;

        let logout_response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/auth/logout")
                .insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
                .to_request(),
        )
        .await;
        assert_eq!(logout_response.status(), StatusCode::NO_CONTENT);

        let me_response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/auth/me")
                .insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
                .to_request(),
        )
        .await;
        assert_eq!(me_response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            me_response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    #[actix_web::test]
    async fn protected_endpoints_reject_missing_and_unknown_tokens() {
        let app = test::init_service(
            App::new()
                .app_data(app_state())
                .service(logout)
                .service(get_current_user),
        )
        .await;

        for request in [
            test::TestRequest::get().uri("/auth/me").to_request(),
            test::TestRequest::post()
                .uri("/auth/logout")
                .insert_header((header::AUTHORIZATION, "Bearer unknown"))
                .to_request(),
        ] {
            let response = test::call_service(&app, request).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/json"
            );
        }
    }
}
