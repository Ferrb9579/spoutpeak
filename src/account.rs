//! Spoutpeak accounts: sign up / sign in with email, name and password
//! against the watch-together server (`watch-server/src/auth.rs`).
//!
//! Only the session token and the account's public details are kept, in
//! the profile; the password never touches the disk.

use crate::state::{App, spawn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The signed-in account, saved in the profile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub token: String,
    pub id: i64,
    pub email: String,
    pub name: String,
}

/// The account server's HTTP address, from the watch-together WebSocket
/// URL (`wss://host/ws` -> `https://host`).
pub fn server_base() -> String {
    let url = crate::together::SERVER_URL;
    let url = url
        .strip_prefix("wss://")
        .map(|rest| format!("https://{rest}"))
        .or_else(|| url.strip_prefix("ws://").map(|rest| format!("http://{rest}")))
        .unwrap_or_else(|| url.to_owned());
    url.trim_end_matches('/').trim_end_matches("/ws").to_owned()
}

/// Passwords cross the network only encrypted, except to this computer
/// (a local test server).
pub fn transport_is_safe() -> bool {
    let base = server_base();
    base.starts_with("https://")
        || ["http://127.0.0.1", "http://localhost", "http://[::1]"].iter().any(|local| {
            base.strip_prefix(local).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        })
}

#[derive(Debug)]
enum Failure {
    /// The server answered with a reason to show.
    Refused { status: u16, message: String },
    Network(String),
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Failure::Refused { message, .. } => message.clone(),
            Failure::Network(error) => format!("Could not reach the account server: {error}"),
        }
    }
}

async fn call(path: &str, token: Option<String>, body: Option<Value>) -> Result<Value, Failure> {
    let url = format!("{}{path}", server_base());
    let client = crate::addons::http();
    let mut request = match &body {
        Some(body) => client.post(&url).json(body),
        None if path == "/auth/logout" => client.post(&url),
        None => client.get(&url),
    };
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| Failure::Network(e.to_string()))?;
    let status = response.status().as_u16();
    if status == 204 {
        return Ok(Value::Null);
    }
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        let message = value["message"].as_str().map(str::to_owned).unwrap_or_else(|| match status {
            404 => "This server does not support accounts yet".to_owned(),
            _ => format!("The account server answered {status}"),
        });
        Err(Failure::Refused { status, message })
    }
}

fn account_from(value: &Value) -> Option<Account> {
    let user = &value["user"];
    Some(Account {
        token: value["token"].as_str()?.to_owned(),
        id: user["id"].as_i64()?,
        email: user["email"].as_str()?.to_owned(),
        name: user["name"].as_str()?.to_owned(),
    })
}

/// Which form the Account page shows while signed out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthMode {
    #[default]
    SignIn,
    SignUp,
}

/// The Account page's progress, for its buttons and messages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuthState {
    pub mode: AuthMode,
    pub busy: bool,
    /// Why the last attempt failed.
    pub error: Option<String>,
}

impl App {
    pub fn account(&self) -> Option<Account> {
        self.profile.read(|p| p.account.clone())
    }

    pub fn signed_in(&self) -> bool {
        self.profile.peek(|p| p.account.is_some())
    }

    fn auth_begin(&self) -> bool {
        if self.auth.peek(|a| a.busy) {
            return false;
        }
        if !transport_is_safe() {
            self.auth.write(|a| {
                a.error = Some(
                    "The account server isn't using HTTPS, so your password would travel unencrypted. \
                     Accounts stay off until it does."
                        .into(),
                )
            });
            return false;
        }
        self.auth.write(|a| {
            a.busy = true;
            a.error = None;
        });
        true
    }

    fn auth_finished(&self, result: Result<Value, Failure>, welcome: &str) {
        match result.map(|value| account_from(&value)) {
            Ok(Some(account)) => {
                let name = account.name.clone();
                self.profile.write(|p| p.account = Some(account));
                self.save_profile();
                self.controllers.account_password.set_text("");
                self.auth.write(|a| {
                    a.busy = false;
                    a.error = None;
                });
                self.notify(format!("{welcome}, {name}"));
            }
            Ok(None) => self.auth.write(|a| {
                a.busy = false;
                a.error = Some("The account server sent an unexpected answer".into());
            }),
            Err(failure) => self.auth.write(|a| {
                a.busy = false;
                a.error = Some(failure.message());
            }),
        }
    }

    pub fn sign_up(&self) {
        let email = self.controllers.account_email.text().trim().to_owned();
        let name = self.controllers.account_name.text().trim().to_owned();
        let password = self.controllers.account_password.text();
        let problem = if email.is_empty() || !email.contains('@') {
            Some("Enter your email address")
        } else if name.is_empty() {
            Some("Enter a name")
        } else if password.chars().count() < 8 {
            Some("Use a password of at least 8 characters")
        } else {
            None
        };
        if let Some(problem) = problem {
            self.auth.write(|a| a.error = Some(problem.into()));
            return;
        }
        if !self.auth_begin() {
            return;
        }
        let body = json!({ "email": email, "name": name, "password": password });
        spawn(call("/auth/signup", None, Some(body)), |app, result| {
            app.auth_finished(result, "Welcome")
        });
    }

    pub fn sign_in(&self) {
        let email = self.controllers.account_email.text().trim().to_owned();
        let password = self.controllers.account_password.text();
        if email.is_empty() || password.is_empty() {
            self.auth.write(|a| a.error = Some("Enter your email and password".into()));
            return;
        }
        if !self.auth_begin() {
            return;
        }
        let body = json!({ "email": email, "password": password });
        spawn(call("/auth/login", None, Some(body)), |app, result| {
            app.auth_finished(result, "Welcome back")
        });
    }

    /// Signs out here and ends the session on the server.
    pub fn sign_out(&self) {
        let Some(account) = self.account() else { return };
        self.together_leave();
        self.profile.write(|p| p.account = None);
        self.save_profile();
        self.notify("Signed out");
        // Best effort: the token is gone from this computer either way.
        spawn(call("/auth/logout", Some(account.token), None), |_, _| {});
    }

    /// At startup: checks the saved session still works and picks up a
    /// renamed account. Network trouble keeps the session as it is.
    pub fn refresh_account(&self) {
        let Some(account) = self.account() else { return };
        spawn(call("/auth/me", Some(account.token.clone()), None), move |app, result| match result {
            Ok(value) => {
                let user = &value["user"];
                let name = user["name"].as_str().unwrap_or(&account.name).to_owned();
                let email = user["email"].as_str().unwrap_or(&account.email).to_owned();
                if name != account.name || email != account.email {
                    app.profile.write(|p| {
                        if let Some(saved) = p.account.as_mut() {
                            saved.name = name;
                            saved.email = email;
                        }
                    });
                    app.save_profile();
                }
            }
            Err(Failure::Refused { status: 401, .. }) => {
                app.profile.write(|p| p.account = None);
                app.save_profile();
                app.notify("Your session ended. Sign in again.");
            }
            Err(_) => {}
        });
    }

    pub fn set_auth_mode(&self, mode: AuthMode) {
        self.auth.write(|a| {
            a.mode = mode;
            a.error = None;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_server_answer() {
        let value = json!({ "token": "t", "user": { "id": 7, "email": "a@b.co", "name": "Ann" } });
        let account = account_from(&value).unwrap();
        assert_eq!((account.id, account.name.as_str(), account.token.as_str()), (7, "Ann", "t"));
        assert!(account_from(&json!({ "user": {} })).is_none());
    }

    #[test]
    fn profile_without_an_account_still_loads() {
        let profile: crate::store::Profile = serde_json::from_str("{}").unwrap();
        assert!(profile.account.is_none());
        let saved = serde_json::to_string(&crate::store::Profile {
            account: Some(Account { token: "x".into(), id: 1, email: "a@b.co".into(), name: "A".into() }),
            ..Default::default()
        })
        .unwrap();
        let back: crate::store::Profile = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.account.unwrap().name, "A");
    }
}
