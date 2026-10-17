//! The Lists plugin's API routes, mounted by the base under
//! `/api/plugins/com.channelflow.lists`.
//!
//! Lists are named collections of catalog items (v1.0.0's list registry,
//! where lists were backed by Jellyfin playlists — here each list is a named
//! set of Media-catalog items, stored in the plugin's own storage, and can be
//! reused across channels and presets).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use channelflow_plugin_api::plugin::PluginLogger;
use channelflow_plugin_api::storage::PluginStorage;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

const LISTS_KEY: &str = "lists";

pub struct RoutesState {
    pub storage: Arc<dyn PluginStorage>,
    pub logger: PluginLogger,
}

pub fn router(storage: Arc<dyn PluginStorage>, logger: PluginLogger) -> Router {
    Router::new()
        .route("/lists", get(list_lists).post(create_list))
        .route("/lists/{id}", put(rename_list).delete(delete_list))
        .route("/lists/{id}/items", post(add_item))
        .route("/lists/{id}/items/{match_key}", delete(remove_item))
        .with_state(Arc::new(RoutesState { storage, logger }))
}

async fn load(state: &RoutesState) -> Value {
    state
        .storage
        .get(LISTS_KEY)
        .await
        .map(|value| value.unwrap_or_else(|| json!({ "lists": [] })))
        .unwrap_or_else(|_| json!({ "lists": [] }))
}

async fn save(state: &RoutesState, value: &Value) -> Result<(), String> {
    state
        .storage
        .set(LISTS_KEY, value)
        .await
        .map_err(|error| error.to_string())
}

fn find_list<'a>(doc: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    doc["lists"]
        .as_array_mut()?
        .iter_mut()
        .find(|list| list["id"].as_str() == Some(id))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Every list, newest first (items included so the page can render counts).
async fn list_lists(State(state): State<Arc<RoutesState>>) -> impl IntoResponse {
    state.logger.info("lists: listing all lists");
    let doc = load(&state).await;
    let mut lists = doc["lists"].as_array().cloned().unwrap_or_default();
    lists.sort_by(|a, b| {
        b["created_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a["created_at"].as_str().unwrap_or(""))
    });
    state.logger.info(&format!("lists: returning {} lists", lists.len()));
    Json(json!({ "lists": lists }))
}

#[derive(Deserialize)]
struct CreateList {
    name: String,
}

async fn create_list(
    State(state): State<Arc<RoutesState>>,
    Json(input): Json<CreateList>,
) -> Response {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        state.logger.warn("lists: create_list failed - name is empty");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name is required" })),
        )
            .into_response();
    }
    let mut doc = load(&state).await;
    let list = json!({
        "id": Uuid::new_v4().to_string(),
        "name": name,
        "created_at": now(),
        "items": [],
    });
    state.logger.info(&format!("lists: creating list '{}' with id {}", name, list["id"]));
    doc["lists"].as_array_mut().map(|lists| lists.push(list.clone()));
    if let Err(message) = save(&state, &doc).await {
        state.logger.error(&format!("lists: failed to save list '{}': {}", name, message));
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
            .into_response();
    }
    state.logger.info(&format!("lists: successfully created list '{}'", name));
    Json(list).into_response()
}

#[derive(Deserialize)]
struct RenameList {
    name: String,
}

async fn rename_list(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(input): Json<RenameList>,
) -> Response {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        state.logger.warn("lists: rename_list failed - name is empty");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name is required" })),
        )
            .into_response();
    }
    state.logger.info(&format!("lists: renaming list {} to '{}'", id, name));
    let mut doc = load(&state).await;
    let Some(list) = find_list(&mut doc, &id) else {
        state.logger.warn(&format!("lists: list {} not found for rename", id));
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such list" }))).into_response();
    };
    list["name"] = json!(name);
    if let Err(message) = save(&state, &doc).await {
        state.logger.error(&format!("lists: failed to save renamed list {}: {}", id, message));
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
            .into_response();
    }
    state.logger.info(&format!("lists: successfully renamed list {} to '{}'", id, name));
    Json(json!({ "ok": true })).into_response()
}

async fn delete_list(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
) -> Response {
    state.logger.info(&format!("lists: deleting list {}", id));
    let mut doc = load(&state).await;
    let mut found = false;
    if let Some(lists) = doc["lists"].as_array_mut() {
        let before = lists.len();
        lists.retain(|list| list["id"].as_str() != Some(&id));
        found = lists.len() != before;
    }
    if !found {
        state.logger.warn(&format!("lists: list {} not found for deletion", id));
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such list" }))).into_response();
    }
    if let Err(message) = save(&state, &doc).await {
        state.logger.error(&format!("lists: failed to save after deleting list {}: {}", id, message));
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
            .into_response();
    }
    state.logger.info(&format!("lists: successfully deleted list {}", id));
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct AddItem {
    #[serde(default)]
    match_key: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    year: Option<i64>,
}

async fn add_item(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(input): Json<AddItem>,
) -> Response {
    if input.match_key.is_empty() || input.title.is_empty() {
        state.logger.warn(&format!("lists: add_item to {} failed - match_key or title is empty", id));
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "match_key and title are required" })),
        )
            .into_response();
    }
    state.logger.info(&format!(
        "lists: adding item '{}' (kind: {}) to list {}",
        input.title, input.kind, id
    ));
    let mut doc = load(&state).await;
    let Some(list) = find_list(&mut doc, &id) else {
        state.logger.warn(&format!("lists: list {} not found for add_item", id));
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such list" }))).into_response();
    };
    let items = list["items"].as_array_mut().expect("lists carry an items array");
    let item = json!({
        "match_key": input.match_key,
        "kind": input.kind,
        "title": input.title,
        "year": input.year,
    });
    if !items.iter().any(|existing| existing["match_key"] == item["match_key"]) {
        items.push(item);
        state.logger.info(&format!("lists: added item with match_key {} to list {}", input.match_key, id));
    } else {
        state.logger.info(&format!("lists: item with match_key {} already exists in list {}, skipping", input.match_key, id));
    }
    if let Err(message) = save(&state, &doc).await {
        state.logger.error(&format!("lists: failed to save after adding item to list {}: {}", id, message));
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
            .into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

async fn remove_item(
    State(state): State<Arc<RoutesState>>,
    Path((id, match_key)): Path<(String, String)>,
) -> Response {
    state.logger.info(&format!("lists: removing item with match_key {} from list {}", match_key, id));
    let mut doc = load(&state).await;
    let Some(list) = find_list(&mut doc, &id) else {
        state.logger.warn(&format!("lists: list {} not found for remove_item", id));
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such list" }))).into_response();
    };
    let mut removed = false;
    if let Some(items) = list["items"].as_array_mut() {
        let before = items.len();
        items.retain(|existing| existing["match_key"].as_str() != Some(match_key.as_str()));
        removed = items.len() != before;
    }
    if !removed {
        state.logger.warn(&format!("lists: item with match_key {} not found in list {}", match_key, id));
    } else {
        state.logger.info(&format!("lists: removed item with match_key {} from list {}", match_key, id));
    }
    if let Err(message) = save(&state, &doc).await {
        state.logger.error(&format!("lists: failed to save after removing item from list {}: {}", id, message));
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
            .into_response();
    }
    Json(json!({ "ok": true })).into_response()
}