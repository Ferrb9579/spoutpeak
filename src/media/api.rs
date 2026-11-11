//! Client for the Media account API (`api.strem.io`): login, addon
//! collection sync and the library datastore.

use super::addon::http;
use super::types::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const API: &str = "https://api.strem.io/api";

async fn call<T: DeserializeOwned>(method: &str, body: Value) -> Result<T, String> {
    let response = http()
        .post(format!("{API}/{method}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Media API unreachable: {e}"))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("Media API returned an invalid response: {e}"))?;
    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(message.to_owned());
    }
    let result = value.get("result").cloned().unwrap_or(Value::Null);
    serde_json::from_value(result).map_err(|e| format!("unexpected Media API result: {e}"))
}

pub async fn login(email: &str, password: &str) -> Result<Auth, String> {
    call(
        "login",
        json!({ "type": "Login", "email": email, "password": password, "facebook": false }),
    )
    .await
}

pub async fn register(email: &str, password: &str) -> Result<Auth, String> {
    call(
        "register",
        json!({
            "type": "Register",
            "email": email,
            "password": password,
            "gdpr_consent": { "tos": true, "privacy": true, "marketing": false, "from": "spoutpeak" }
        }),
    )
    .await
}

pub async fn logout(auth_key: &str) -> Result<Value, String> {
    call("logout", json!({ "type": "Logout", "authKey": auth_key })).await
}

pub async fn get_user(auth_key: &str) -> Result<User, String> {
    call("getUser", json!({ "type": "GetUser", "authKey": auth_key })).await
}

#[derive(serde::Deserialize)]
struct AddonCollection {
    #[serde(default)]
    addons: Vec<Descriptor>,
}

pub async fn addon_collection_get(auth_key: &str) -> Result<Vec<Descriptor>, String> {
    let collection: AddonCollection = call(
        "addonCollectionGet",
        json!({ "type": "AddonCollectionGet", "authKey": auth_key, "update": true }),
    )
    .await?;
    Ok(collection.addons)
}

pub async fn addon_collection_set(auth_key: &str, addons: &[Descriptor]) -> Result<Value, String> {
    call(
        "addonCollectionSet",
        json!({ "type": "AddonCollectionSet", "authKey": auth_key, "addons": addons }),
    )
    .await
}

pub async fn library_get_all(auth_key: &str) -> Result<Vec<LibraryItem>, String> {
    let items: Vec<Value> = call(
        "datastoreGet",
        json!({
            "authKey": auth_key,
            "collection": "libraryItem",
            "ids": [],
            "all": true
        }),
    )
    .await?;
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

pub async fn library_put(auth_key: &str, changes: &[LibraryItem]) -> Result<Value, String> {
    if changes.is_empty() {
        return Ok(Value::Null);
    }
    call(
        "datastorePut",
        json!({
            "authKey": auth_key,
            "collection": "libraryItem",
            "changes": changes
        }),
    )
    .await
}
