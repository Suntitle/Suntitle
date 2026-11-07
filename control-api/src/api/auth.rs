//! Startup creates the administrator only when absent; stored credentials MUST survive restarts.
//! Successful verification creates a session. Sessions expire or are revoked; restarts revoke all.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, get, post, web};
use scrypt::{
    Scrypt,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use sqlx::Row;
use uuid::Uuid;

use super::errors::{ApiError, ApiResult, BadRequest, ErrorEnvelope, Unauthorized};
use super::models::{LoginRequest, LoginResponse, User};

const TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Shared administrator credentials and process-local bearer sessions.
#[derive(Clone, Default)]
pub struct AuthState {
    /// Absent credentials MUST reject every login.
    admin: Option<Arc<Administrator>>,
    /// Active sessions shared by all HTTP workers.
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

/// Persisted administrator identity and salted password hash.
struct Administrator {
    /// Stable database identity.
    user: User,
    /// PHC encoded scrypt hash. Plaintext MUST NOT be persisted.
    password_hash: String,
}

/// Authenticated identity valid until expiry or explicit revocation.
#[derive(Clone)]
struct Session {
    /// Identity verified at login.
    user: User,
    /// Monotonic expiry deadline.
    expires_at: Instant,
}

impl AuthState {
    /// Load persisted credentials, bootstrapping from the environment only for an empty table.
    /// Concurrent startups MUST serialize initialization before checking for the administrator.
    pub async fn from_database(
        database_url: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await?;
        let state = Self::initialize(&pool, || {
            Ok((
                std::env::var("ADMIN_USERNAME")?,
                std::env::var("ADMIN_PASSWORD")?,
            ))
        })
        .await?;
        pool.close().await;
        Ok(state)
    }

    /// Serialize bootstrap and load the singleton account. Credentials are read only if absent.
    async fn initialize(
        pool: &sqlx::PgPool,
        credentials: impl FnOnce() -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut transaction = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(734820115)")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS suntitle_admin (
                singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
                id TEXT NOT NULL,
                username TEXT NOT NULL CHECK (length(btrim(username)) > 0),
                password_hash TEXT NOT NULL
            )",
        )
        .execute(&mut *transaction)
        .await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM suntitle_admin)")
            .fetch_one(&mut *transaction)
            .await?;
        if !exists {
            let (username, password) = credentials()?;
            if username.trim().is_empty() || password.is_empty() {
                return Err(
                    "ADMIN_USERNAME and ADMIN_PASSWORD MUST be nonempty on first startup".into(),
                );
            }
            let hash = web::block(move || hash_password(&password)).await??;
            sqlx::query(
                "INSERT INTO suntitle_admin (id, username, password_hash) VALUES ($1, $2, $3)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(username)
            .bind(hash)
            .execute(&mut *transaction)
            .await?;
        }
        let row = sqlx::query("SELECT id, username, password_hash FROM suntitle_admin")
            .fetch_one(&mut *transaction)
            .await?;
        let admin = Administrator {
            user: User {
                id: row.try_get::<String, _>("id")?.parse()?,
                username: row.try_get("username")?,
            },
            password_hash: row.try_get("password_hash")?,
        };
        PasswordHash::new(&admin.password_hash)
            .map_err(|_| "Invalid stored administrator password hash")?;
        transaction.commit().await?;
        Ok(Self {
            admin: Some(Arc::new(admin)),
            ..Self::default()
        })
    }

    /// Verify credentials without exposing which credential failed.
    fn authenticate(&self, username: &str, password: &str) -> Option<User> {
        let admin = self.admin.as_ref()?;
        let hash = PasswordHash::new(&admin.password_hash).ok()?;
        let valid = Scrypt::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok();
        (valid && username == admin.user.username).then(|| admin.user.clone())
    }

    /// Create a fresh bearer token for an already verified identity.
    fn create_session(&self, user: User) -> (String, Session) {
        let token = Uuid::new_v4().simple().to_string();
        let session = Session {
            user,
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

    /// Find an active session and discard it if expired.
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

    /// Remove a session; return whether it existed.
    fn revoke(&self, token: &str) -> bool {
        self.sessions
            .write()
            .expect("authentication state lock poisoned")
            .remove(token)
            .is_some()
    }
}

/// Hash a bootstrap password with an independently generated salt.
fn hash_password(password: &str) -> Result<String, String> {
    Scrypt::default()
        .hash_password(password.as_bytes())
        .map(|hash: PasswordHash| hash.to_string())
        .map_err(|error| error.to_string())
}

/// Parse one bearer token from the authorization header.
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
/// Verify administrator credentials before issuing a session.
pub async fn login(
    request: HttpRequest,
    state: web::Data<AuthState>,
    body: web::Json<LoginRequest>,
) -> ApiResult<LoginResponse> {
    let credentials = body.into_inner();
    let verifier = state.clone();
    let user =
        web::block(move || verifier.authenticate(&credentials.username, &credentials.password))
            .await
            .map_err(|_| ApiError::unauthorized(request.path()))?
            .ok_or_else(|| ApiError::unauthorized(request.path()))?;
    let (access_token, session) = state.create_session(user);
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

    /// Persisted PHC hashes MUST remain verifiable after the scrypt upgrade.
    #[actix_web::test]
    async fn legacy_password_hash_remains_valid() {
        let hash = PasswordHash::new(
            "$scrypt$ln=16,r=8,p=1$aM15713r3Xsvxbi31lqr1Q$nFNh2CVHVjNldFVKDHDlm4CbdRSCdEBsjjJxD+iCs5E",
        )
        .unwrap();
        assert!(
            Scrypt::default()
                .verify_password(b"password", &hash)
                .is_ok()
        );
        assert!(
            Scrypt::default()
                .verify_password(b"incorrect", &hash)
                .is_err()
        );
    }

    /// Build an isolated administrator fixture.
    fn app_state() -> web::Data<AuthState> {
        web::Data::new(AuthState {
            admin: Some(Arc::new(Administrator {
                user: User {
                    id: Uuid::new_v4(),
                    username: "alice".to_owned(),
                },
                password_hash: hash_password("anything").unwrap(),
            })),
            ..AuthState::default()
        })
    }

    /// Verify bootstrap and restarts in a disposable schema on a test PostgreSQL server.
    #[actix_web::test]
    #[ignore = "requires TEST_DATABASE_URL and permission to create schemas"]
    async fn database_bootstrap_preserves_credentials() {
        use std::str::FromStr;
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let connection = sqlx::PgPool::connect(&url).await.unwrap();
        // The identifier contains only a fixed prefix and generated hexadecimal digits.
        let schema = format!("auth_test_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&connection)
            .await
            .unwrap();
        let options = sqlx::postgres::PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_with(options)
            .await
            .unwrap();
        assert!(
            AuthState::initialize(&pool, || Ok(("".into(), "".into())))
                .await
                .is_err()
        );
        let first = AuthState::initialize(&pool, || Ok(("admin".into(), "secret".into())))
            .await
            .unwrap();
        let user = first.authenticate("admin", "secret").unwrap();
        let restarted = AuthState::initialize(&pool, || {
            panic!("existing account MUST NOT read bootstrap credentials")
        })
        .await
        .unwrap();
        assert_eq!(
            restarted.authenticate("admin", "secret").unwrap().id,
            user.id
        );
        assert!(restarted.authenticate("admin", "changed").is_none());
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM suntitle_admin")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(hash.starts_with("$scrypt$"));
        assert_ne!(hash, "secret");
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&connection)
            .await
            .unwrap();
        connection.close().await;
    }

    /// Missing bootstrap state MUST never permit a login.
    #[actix_web::test]
    async fn unconfigured_authentication_rejects_login() {
        assert!(
            AuthState::default()
                .authenticate("anyone", "anything")
                .is_none()
        );
    }

    /// Accept only valid credentials and preserve identity across independent sessions.
    #[actix_web::test]
    async fn login_verifies_credentials_and_creates_distinct_tokens() {
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
                    password: "anything".to_owned(),
                })
                .to_request(),
        )
        .await;

        assert_ne!(first.access_token, second.access_token);
        assert_eq!(first.token_type, "Bearer");
        assert_eq!(first.expires_in, 86_400);
        assert_eq!(first.user.username, "alice");
        assert_eq!(first.user.id, second.user.id);
        for (username, password) in [("alice", "wrong"), ("other", "anything"), ("", "")] {
            let response = test::call_service(
                &app,
                test::TestRequest::post()
                    .uri("/auth/login")
                    .set_json(&LoginRequest {
                        username: username.into(),
                        password: password.into(),
                    })
                    .to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

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
        let (token, _) = state.create_session(state.admin.as_ref().unwrap().user.clone());
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
