//! Authentication: a session cookie for the web UI, bearer tokens for
//! scripts and other API clients. Requires the database (users live there);
//! without one, authentication is off and the server only listens on
//! loopback addresses.

use crate::Hub;
use crate::api::ApiError;
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use watcher_store::{ApiToken, TokenRef, User};

pub const SESSION_COOKIE: &str = "watcher_session";
/// Idle timeout of a session; every use extends it.
pub const SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
/// Prefix of API tokens, so they are recognisable in configs and logs.
pub const TOKEN_PREFIX: &str = "wt_";

const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);

/// API paths reachable without logging in.
const PUBLIC: [&str; 4] = [
    "/api/v1/health",
    "/api/v1/openapi.yaml",
    "/api/v1/auth/status",
    "/api/v1/auth/login",
];

/// How the hub authenticates requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthSettings {
    /// Every API request needs a session or token.
    pub required: bool,
    /// Mark the session cookie `Secure` (served over HTTPS).
    pub secure_cookies: bool,
}

impl AuthSettings {
    /// No authentication: only for servers bound to loopback.
    pub const OFF: AuthSettings = AuthSettings {
        required: false,
        secure_cookies: false,
    };
}

/// The logged-in user, available to handlers as an extension.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

/// Hashes a password for storage (Argon2id, random salt).
pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// A random secret and the SHA-256 hash to store for it.
fn new_secret(prefix: &str) -> (String, Vec<u8>) {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator works");
    let secret = format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = secret_hash(&secret);
    (secret, hash)
}

fn secret_hash(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

/// A new API token (`wt_…`) and the hash to store for it.
pub fn new_api_token() -> (String, Vec<u8>) {
    new_secret(TOKEN_PREFIX)
}

/// Failed logins per username, to slow down password guessing.
#[derive(Default)]
pub struct Limiter {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

impl Limiter {
    /// `Err(wait)` while the username is locked out.
    fn check(&self, username: &str) -> Result<(), Duration> {
        let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        match failures.get(&username.to_lowercase()) {
            Some(&(n, last)) if n >= MAX_FAILURES && last.elapsed() < LOCKOUT => {
                Err(LOCKOUT - last.elapsed())
            }
            _ => Ok(()),
        }
    }

    fn fail(&self, username: &str) {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        failures.retain(|_, (_, last)| last.elapsed() < LOCKOUT);
        let entry = failures
            .entry(username.to_lowercase())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
    }

    fn succeed(&self, username: &str) {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        failures.remove(&username.to_lowercase());
    }
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then(|| v.to_string())
        })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// Cookie-authenticated requests that change something must come from this
/// site. SameSite=Strict already prevents cross-site cookies; this is a
/// second line for older browsers.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    origin
        .split_once("://")
        .is_some_and(|(_, rest)| rest == host)
}

/// Middleware: lets public paths and the web UI through, requires a
/// session or token for the rest of `/api`.
pub async fn require(State(hub): State<Arc<Hub>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !hub.auth.required || !path.starts_with("/api/") || PUBLIC.contains(&path) {
        return next.run(req).await;
    }
    let Some(store) = hub.store.as_deref() else {
        return ApiError::Unavailable("authentication needs the database".into()).into_response();
    };

    let headers = req.headers();
    let user = if let Some(token) = bearer(headers) {
        store.token_user(&secret_hash(token)).await
    } else if let Some(session) = cookie(headers, SESSION_COOKIE) {
        if req.method() != Method::GET && req.method() != Method::HEAD && !same_origin(headers) {
            return ApiError::Forbidden("cross-site request refused".into()).into_response();
        }
        store
            .session_user(&secret_hash(&session), SESSION_TTL)
            .await
    } else {
        Ok(None)
    };
    match user {
        Ok(Some(user)) => {
            req.extensions_mut().insert(CurrentUser(user));
            next.run(req).await
        }
        Ok(None) => ApiError::Unauthorized.into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

/// Whether login is needed, and who is logged in. Public.
pub async fn status(
    State(hub): State<Arc<Hub>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(store) = hub.store.as_deref().filter(|_| hub.auth.required) else {
        return Ok(Json(
            json!({ "required": false, "user": null, "users_exist": false }),
        ));
    };
    let user = match cookie(&headers, SESSION_COOKIE) {
        Some(session) => {
            store
                .session_user(&secret_hash(&session), SESSION_TTL)
                .await?
        }
        None => None,
    };
    let users_exist = store.user_count().await? > 0;
    Ok(Json(
        json!({ "required": true, "user": user, "users_exist": users_exist }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    username: String,
    password: String,
}

pub async fn login(
    State(hub): State<Arc<Hub>>,
    Json(body): Json<LoginBody>,
) -> Result<Response, ApiError> {
    let store = hub
        .store
        .clone()
        .filter(|_| hub.auth.required)
        .ok_or_else(|| ApiError::NotFound("authentication is off on this server".into()))?;
    if let Err(wait) = hub.limiter.check(&body.username) {
        return Err(ApiError::TooManyRequests(format!(
            "too many failed logins; try again in {}s",
            wait.as_secs() + 1
        )));
    }

    let found = store.login_user(&body.username).await?;
    // Unknown users cost as much time as wrong passwords, so the response
    // time doesn't reveal which usernames exist.
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = match &found {
        Some((user, hash)) if !user.disabled => hash.clone(),
        _ => DUMMY
            .get_or_init(|| hash_password("not a real password").unwrap_or_default())
            .clone(),
    };
    let password = body.password.clone();
    let valid = tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false);
    let user = match found {
        Some((user, _)) if valid && !user.disabled => user,
        _ => {
            hub.limiter.fail(&body.username);
            tracing::info!("failed login for {:?}", body.username);
            return Err(ApiError::Unauthorized);
        }
    };
    hub.limiter.succeed(&body.username);

    let (session, hash) = new_secret("");
    store.create_session(user.id, &hash, SESSION_TTL).await?;
    let secure = if hub.auth.secure_cookies {
        "; Secure"
    } else {
        ""
    };
    let set_cookie = format!(
        "{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        SESSION_TTL.as_secs()
    );
    let mut response = Json(json!({ "user": user })).into_response();
    if let Ok(value) = HeaderValue::from_str(&set_cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

pub async fn logout(State(hub): State<Arc<Hub>>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let (Some(store), Some(session)) = (hub.store.as_deref(), cookie(&headers, SESSION_COOKIE)) {
        store.delete_session(&secret_hash(&session)).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("watcher_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    Ok(response)
}

fn current(user: Option<Extension<CurrentUser>>) -> Result<User, ApiError> {
    user.map(|Extension(CurrentUser(u))| u)
        .ok_or_else(|| ApiError::NotFound("authentication is off on this server".into()))
}

/// The current user's API tokens.
pub async fn tokens(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
) -> Result<Json<Vec<ApiToken>>, ApiError> {
    let user = current(user)?;
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    Ok(Json(store.tokens(user.id).await?))
}

#[derive(Debug, Deserialize)]
pub struct TokenBody {
    name: String,
}

/// Creates an API token. The token is only shown in this response.
pub async fn create_token(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
    Json(body): Json<TokenBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let user = current(user)?;
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::BadRequest(
            "give the token a name (up to 100 characters)".into(),
        ));
    }
    let (secret, hash) = new_api_token();
    let token = store.create_token(user.id, name, &hash).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "token": secret, "info": token })),
    ))
}

pub async fn delete_token(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<i32>,
) -> Result<StatusCode, ApiError> {
    let user = current(user)?;
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    match store.delete_token(user.id, TokenRef::Id(id)).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::NotFound(format!("no token {id}"))),
    }
}

/// Admins only; without login, anyone (the server is on loopback then).
pub(crate) fn admin_user(
    hub: &Hub,
    user: &Option<Extension<CurrentUser>>,
) -> Result<String, ApiError> {
    match user {
        Some(Extension(CurrentUser(u))) if u.admin => Ok(u.username.clone()),
        None if !hub.auth.required => Ok("local user".into()),
        _ => Err(ApiError::Forbidden("admin rights required".into())),
    }
}

/// All user accounts. Admins only.
pub async fn users(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
) -> Result<Json<Vec<User>>, ApiError> {
    admin_user(&hub, &user)?;
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    Ok(Json(store.users().await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewUser {
    username: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

/// Creates a user. Admins only.
pub async fn create_user(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
    Json(body): Json<NewUser>,
) -> Result<(StatusCode, Json<User>), ApiError> {
    let by = admin_user(&hub, &user)?;
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    let name = body.username.trim();
    if name.is_empty()
        || name.len() > 64
        || name.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::BadRequest(
            "usernames are 1 to 64 characters without spaces".into(),
        ));
    }
    check_password(&body.password)?;
    let hash = hash_password(&body.password).map_err(ApiError::Internal)?;
    let created = store.create_user(name, &hash, body.admin).await?;
    tracing::info!("{by}: created user {name}");
    Ok((StatusCode::CREATED, Json(created)))
}

/// Shortest password accepted anywhere (API, CLI, setup).
pub const MIN_PASSWORD: usize = 10;

fn check_password(password: &str) -> Result<(), ApiError> {
    match password.chars().count() < MIN_PASSWORD {
        true => Err(ApiError::BadRequest(format!(
            "password: at least {MIN_PASSWORD} characters"
        ))),
        false => Ok(()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserChange {
    password: Option<String>,
    admin: Option<bool>,
    disabled: Option<bool>,
}

/// Changes a user: admins anything, users their own password.
pub async fn update_user(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
    Path(name): Path<String>,
    Json(body): Json<UserChange>,
) -> Result<Json<User>, ApiError> {
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    let own = user
        .as_ref()
        .is_some_and(|Extension(CurrentUser(u))| u.username.eq_ignore_ascii_case(&name));
    let only_password = body.admin.is_none() && body.disabled.is_none();
    let by = match (own && only_password, admin_user(&hub, &user)) {
        (true, _) => name.clone(),
        (false, Ok(by)) => by,
        (false, Err(e)) => return Err(e),
    };
    if own && body.disabled == Some(true) {
        return Err(ApiError::BadRequest("you can't disable yourself".into()));
    }
    let not_found = || ApiError::NotFound(format!("no user {name:?}"));
    if let Some(password) = &body.password {
        check_password(password)?;
        let hash = hash_password(password).map_err(ApiError::Internal)?;
        if !store.set_password(&name, &hash).await? {
            return Err(not_found());
        }
    }
    let updated = store
        .update_user(&name, body.admin, body.disabled)
        .await?
        .ok_or_else(not_found)?;
    tracing::info!("{by}: changed user {name}");
    Ok(Json(updated))
}

/// Deletes a user with their sessions and tokens. Admins only, not
/// themselves.
pub async fn delete_user(
    State(hub): State<Arc<Hub>>,
    user: Option<Extension<CurrentUser>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let by = admin_user(&hub, &user)?;
    if by.eq_ignore_ascii_case(&name) {
        return Err(ApiError::BadRequest("you can't delete yourself".into()));
    }
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    if !store.delete_user(&name).await? {
        return Err(ApiError::NotFound(format!("no user {name:?}")));
    }
    tracing::info!("{by}: deleted user {name}");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("correct horse", "not a hash"));
    }

    #[test]
    fn secrets_are_random_and_hashed() {
        let (a, hash_a) = new_api_token();
        let (b, _) = new_api_token();
        assert!(a.starts_with("wt_") && a.len() == 3 + 43);
        assert_ne!(a, b);
        assert_eq!(hash_a, secret_hash(&a));
        assert_eq!(hash_a.len(), 32);
    }

    #[test]
    fn locks_out_after_repeated_failures() {
        let limiter = Limiter::default();
        for _ in 0..MAX_FAILURES {
            assert!(limiter.check("Admin").is_ok());
            limiter.fail("admin");
        }
        assert!(limiter.check("ADMIN").is_err());
        limiter.succeed("admin");
        assert!(limiter.check("admin").is_ok());
    }

    #[test]
    fn reads_cookies_and_origins() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; watcher_session=abc; b=2"),
        );
        assert_eq!(cookie(&h, SESSION_COOKIE).as_deref(), Some("abc"));
        h.insert(
            header::HOST,
            HeaderValue::from_static("watcher.example:8080"),
        );
        assert!(same_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://watcher.example:8080"),
        );
        assert!(same_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!same_origin(&h));
    }
}
