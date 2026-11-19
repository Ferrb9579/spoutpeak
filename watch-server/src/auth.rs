//! Accounts: sign up and sign in with email, name and password.
//!
//! Stored in a Postgres-compatible database (pgrust, or Postgres itself),
//! reached through `DATABASE_URL`. Passwords are kept only as Argon2id
//! hashes. A sign-in hands out a random session token; the database keeps
//! only its SHA-256, so a leaked table cannot be replayed.
//!
//! HTTP, JSON bodies:
//! - `POST /auth/signup {email, name, password}` -> `{token, user}`
//! - `POST /auth/login {email, password}` -> `{token, user}`
//! - `POST /auth/logout` (Authorization: Bearer) -> 204
//! - `GET /auth/me` (Authorization: Bearer) -> `{user}`
//!
//! Errors answer `{error, message}` with 400 (bad input), 401 (wrong email or
//! password, or a dead session), 409 (email taken), 429 (too many tries) or
//! 503 (the database is unreachable).

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_postgres::{Client, NoTls};

/// Sessions not used for this long expire.
const SESSION_DAYS: i64 = 90;
const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 128;
const MAX_NAME: usize = 32;
const MAX_EMAIL: usize = 254;
/// Failed sign-ins allowed per address within [`WINDOW`].
const LOGIN_TRIES: u32 = 10;
/// Accounts made per address within [`WINDOW`].
const SIGNUPS: u32 = 5;
const WINDOW: Duration = Duration::from_secs(15 * 60);

/// The schema's history: SQL files in `migrations/`, compiled in and
/// applied in version order by refinery, which records each one in
/// `refinery_schema_history`.
mod embedded {
    refinery::embed_migrations!("migrations");
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub name: String,
}

impl User {
    fn json(&self) -> Value {
        json!({ "id": self.id, "email": self.email, "name": self.name })
    }
}

/// The database, reconnected on demand when the connection drops.
pub struct Accounts {
    url: String,
    client: tokio::sync::Mutex<Option<Arc<Client>>>,
    /// Per address: (attempts, window start), for sign-ins and sign-ups.
    logins: Mutex<HashMap<String, (u32, Instant)>>,
    signups: Mutex<HashMap<String, (u32, Instant)>>,
}

pub type Shared = Arc<Accounts>;

impl Accounts {
    /// Connects and creates the tables when missing.
    pub async fn open(url: String) -> Result<Shared, String> {
        let accounts = Arc::new(Accounts {
            url,
            client: tokio::sync::Mutex::new(None),
            logins: Mutex::default(),
            signups: Mutex::default(),
        });
        accounts.migrate().await?;
        Ok(accounts)
    }

    /// Brings the database up to the newest migration, on a connection of
    /// its own (refinery needs it exclusively).
    async fn migrate(&self) -> Result<(), String> {
        let (mut client, connection) =
            tokio_postgres::connect(&self.url, NoTls).await.map_err(|e| format!("database: {e}"))?;
        let driver = tokio::spawn(connection);
        let report = embedded::migrations::runner()
            .run_async(&mut client)
            .await
            .map_err(|e| format!("migrating the database: {e}"))?;
        for migration in report.applied_migrations() {
            println!("Applied migration {migration}");
        }
        drop(client);
        let _ = driver.await;
        Ok(())
    }

    async fn client(&self) -> Result<Arc<Client>, String> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref().filter(|c| !c.is_closed()) {
            return Ok(client.clone());
        }
        let (client, connection) =
            tokio_postgres::connect(&self.url, NoTls).await.map_err(|e| format!("database: {e}"))?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                eprintln!("database connection ended: {error}");
            }
        });
        let client = Arc::new(client);
        *slot = Some(client.clone());
        Ok(client)
    }

    /// The account a session token belongs to; refreshes its last use.
    pub async fn user_for_token(&self, token: &str) -> Result<Option<User>, String> {
        if token.is_empty() || token.len() > 128 {
            return Ok(None);
        }
        let client = self.client().await?;
        let now = unix_now();
        let row = client
            .query_opt(
                "UPDATE sessions SET last_used = $2 WHERE token_hash = $1 AND last_used > $3 \
                 RETURNING account_id",
                &[&hash_token(token), &now, &(now - SESSION_DAYS * 86_400)],
            )
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = row else { return Ok(None) };
        let id: i64 = row.get(0);
        let row = client
            .query_opt("SELECT email, name FROM accounts WHERE id = $1", &[&id])
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.map(|row| User { id, email: row.get(0), name: row.get(1) }))
    }

    async fn new_session(&self, client: &Client, account: i64) -> Result<String, String> {
        let token = random_token();
        let now = unix_now();
        client
            .execute(
                "INSERT INTO sessions (token_hash, account_id, created_at, last_used) VALUES ($1, $2, $3, $3)",
                &[&hash_token(&token), &account, &now],
            )
            .await
            .map_err(|e| e.to_string())?;
        // Expired sessions of this account go while we're here.
        let _ = client
            .execute(
                "DELETE FROM sessions WHERE account_id = $1 AND last_used <= $2",
                &[&account, &(now - SESSION_DAYS * 86_400)],
            )
            .await;
        Ok(token)
    }
}

/// Counts an attempt; false once the address has used up its window.
fn allow(table: &Mutex<HashMap<String, (u32, Instant)>>, key: &str, limit: u32) -> bool {
    let mut table = table.lock().unwrap();
    table.retain(|_, (_, start)| start.elapsed() < WINDOW);
    let entry = table.entry(key.to_owned()).or_insert((0, Instant::now()));
    entry.0 += 1;
    entry.0 <= limit
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// 32 bytes from the operating system's secure generator.
fn random_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system random numbers");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_password(password: String) -> Result<String, String> {
    // Argon2id with a fresh random salt from the operating system.
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

fn password_matches(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

/// A valid hash of nothing in particular: checked against when the email is
/// unknown, so a wrong email takes as long as a wrong password.
fn decoy_hash() -> &'static str {
    static DECOY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DECOY.get_or_init(|| hash_password("decoy password".into()).unwrap_or_default())
}

pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && email.len() <= MAX_EMAIL
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(email)
}

pub fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    (!name.is_empty() && name.chars().count() <= MAX_NAME).then_some(name)
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

/// The server runs without a database.
fn disabled() -> Response {
    fail(StatusCode::SERVICE_UNAVAILABLE, "disabled", "This server has no accounts")
}

fn unavailable(error: String) -> Response {
    eprintln!("accounts: {error}");
    fail(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Accounts are unavailable right now. Try again soon.")
}

/// The client's address, as the proxy in front of us reports it.
fn peer(headers: &HeaderMap, socket: SocketAddr) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_owned())
        .unwrap_or_else(|| socket.ip().to_string())
}

pub fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[derive(Deserialize)]
pub struct SignUp {
    #[serde(default)]
    email: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    password: String,
}

#[derive(Deserialize)]
pub struct LogIn {
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
}

pub async fn signup(
    State(accounts): State<Option<Shared>>,
    ConnectInfo(socket): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<SignUp>,
) -> Response {
    let Some(accounts) = accounts else { return disabled() };
    let Some(email) = normalize_email(&body.email) else {
        return fail(StatusCode::BAD_REQUEST, "email", "Enter a valid email address");
    };
    let Some(name) = clean_name(&body.name) else {
        return fail(StatusCode::BAD_REQUEST, "name", "Enter a name of up to 32 characters");
    };
    let length = body.password.chars().count();
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&length) {
        return fail(StatusCode::BAD_REQUEST, "password", "Use a password of 8 to 128 characters");
    }
    if !allow(&accounts.signups, &peer(&headers, socket), SIGNUPS) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "throttled", "Too many new accounts. Try again later.");
    }
    let hash = match tokio::task::spawn_blocking(move || hash_password(body.password)).await {
        Ok(Ok(hash)) => hash,
        _ => return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Could not create the account"),
    };
    let client = match accounts.client().await {
        Ok(client) => client,
        Err(error) => return unavailable(error),
    };
    let row = client
        .query_opt(
            "INSERT INTO accounts (email, name, password_hash, created_at) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (email) DO NOTHING RETURNING id",
            &[&email, &name, &hash, &unix_now()],
        )
        .await;
    let id: i64 = match row {
        Ok(Some(row)) => row.get(0),
        Ok(None) => return fail(StatusCode::CONFLICT, "taken", "An account with this email already exists"),
        Err(error) => return unavailable(error.to_string()),
    };
    match accounts.new_session(&client, id).await {
        Ok(token) => Json(json!({ "token": token, "user": User { id, email, name }.json() })).into_response(),
        Err(error) => unavailable(error),
    }
}

pub async fn login(
    State(accounts): State<Option<Shared>>,
    ConnectInfo(socket): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LogIn>,
) -> Response {
    let Some(accounts) = accounts else { return disabled() };
    let address = peer(&headers, socket);
    if accounts.logins.lock().unwrap().get(&address).is_some_and(|(n, t)| *n >= LOGIN_TRIES && t.elapsed() < WINDOW)
    {
        return fail(StatusCode::TOO_MANY_REQUESTS, "throttled", "Too many attempts. Try again in a few minutes.");
    }
    let email = normalize_email(&body.email).unwrap_or_default();
    let client = match accounts.client().await {
        Ok(client) => client,
        Err(error) => return unavailable(error),
    };
    let row = match client
        .query_opt("SELECT id, name, password_hash FROM accounts WHERE email = $1", &[&email])
        .await
    {
        Ok(row) => row,
        Err(error) => return unavailable(error.to_string()),
    };
    let found = row.map(|row| (row.get::<_, i64>(0), row.get::<_, String>(1), row.get::<_, String>(2)));
    let hash = found.as_ref().map_or_else(|| decoy_hash().to_owned(), |(_, _, hash)| hash.clone());
    let password = body.password;
    let matches = tokio::task::spawn_blocking(move || password_matches(&password, &hash)).await.unwrap_or(false);
    let Some((id, name, _)) = found.filter(|_| matches) else {
        allow(&accounts.logins, &address, LOGIN_TRIES);
        return fail(StatusCode::UNAUTHORIZED, "credentials", "Wrong email or password");
    };
    match accounts.new_session(&client, id).await {
        Ok(token) => Json(json!({ "token": token, "user": User { id, email, name }.json() })).into_response(),
        Err(error) => unavailable(error),
    }
}

pub async fn logout(State(accounts): State<Option<Shared>>, headers: HeaderMap) -> Response {
    let Some(accounts) = accounts else { return disabled() };
    let token = bearer(&headers);
    if token.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let client = match accounts.client().await {
        Ok(client) => client,
        Err(error) => return unavailable(error),
    };
    match client.execute("DELETE FROM sessions WHERE token_hash = $1", &[&hash_token(&token)]).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => unavailable(error.to_string()),
    }
}

pub async fn me(State(accounts): State<Option<Shared>>, headers: HeaderMap) -> Response {
    let Some(accounts) = accounts else { return disabled() };
    match accounts.user_for_token(&bearer(&headers)).await {
        Ok(Some(user)) => Json(json!({ "user": user.json() })).into_response(),
        Ok(None) => fail(StatusCode::UNAUTHORIZED, "session", "Signed out: sign in again"),
        Err(error) => unavailable(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_checked_and_lowercased() {
        assert_eq!(normalize_email("  Ann@Example.COM "), Some("ann@example.com".into()));
        for bad in ["", "ann", "ann@", "@example.com", "ann@example", "a b@example.com", "ann@.com", "a@b@c.com"] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
    }

    #[test]
    fn names_are_trimmed_and_limited() {
        assert_eq!(clean_name("  Ann "), Some("Ann".into()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(&"x".repeat(33)), None);
    }

    #[test]
    fn passwords_round_trip_through_argon2() {
        let hash = hash_password("correct horse".into()).unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(password_matches("correct horse", &hash));
        assert!(!password_matches("wrong horse", &hash));
        assert!(!password_matches("anything", decoy_hash()) || decoy_hash().is_empty());
    }

    #[test]
    fn tokens_are_random_and_stored_hashed() {
        let (a, b) = (random_token(), random_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert_eq!(hash_token(&a).len(), 64);
        assert_ne!(hash_token(&a), a);
    }

    #[test]
    fn throttle_stops_after_the_limit() {
        let table = Mutex::default();
        assert!((0..3).all(|_| allow(&table, "1.2.3.4", 3)));
        assert!(!allow(&table, "1.2.3.4", 3));
        assert!(allow(&table, "5.6.7.8", 3));
    }
}
