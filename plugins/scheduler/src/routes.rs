//! The Scheduler plugin's HTTP API: settings, the per-channel layers, the rule
//! scopes, the compile dry-run, and the build status.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use channelflow_plugin_api::core::{CatalogFilter, CoreData};
use channelflow_plugin_api::plugin::PluginLogger;
use channelflow_plugin_api::storage::PluginStorage;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::layers::{self, ItemsLayer, PoolItem, ResolvedLayers, Rule, RuleScope};
use crate::settings::{
    layer_key, read_scopes, write_scopes, SchedulerSettings, SETTINGS_KEY,
};

/// What the route handlers need.
pub struct RoutesState {
    pub storage: Arc<dyn PluginStorage>,
    pub core: Arc<dyn CoreData>,
    pub settings: Mutex<SchedulerSettings>,
    #[allow(dead_code)]
    pub logger: PluginLogger,
}

pub fn router(state: Arc<RoutesState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/channels/{id}/hours", get(get_hours).put(put_hours))
        .route("/channels/{id}/rules", get(get_rules).put(put_rules))
        .route("/channels/{id}/items", get(get_items).put(put_items))
        .route("/channels/{id}/prompt", get(get_prompt).put(put_prompt))
        .route("/channels/{id}/compile", post(compile))
        .route("/scopes", get(get_scopes).post(add_scope))
        .route("/scopes/{id}", put(update_scope).delete(delete_scope))
        .route("/build/status", get(build_status))
        .with_state(state)
}

fn fail(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

// ── settings ─────────────────────────────────────────────────────────────────

async fn get_settings(State(state): State<Arc<RoutesState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(json!({ "settings": settings })).into_response()
}

async fn put_settings(State(state): State<Arc<RoutesState>>, Json(value): Json<Value>) -> Response {
    let settings = match SchedulerSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    Json(json!({ "settings": settings })).into_response()
}

// ── per-channel layers ───────────────────────────────────────────────────────

async fn read_layer(state: &RoutesState, channel: &str, layer: &str) -> Value {
    state
        .storage
        .get(&layer_key(channel, layer))
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null)
}

async fn write_layer(
    state: &RoutesState,
    channel: &str,
    layer: &str,
    value: &Value,
) -> Result<(), String> {
    state
        .storage
        .set(&layer_key(channel, layer), value)
        .await
        .map_err(|error| error.to_string())
}

async fn get_hours(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(read_layer(&state, &id, "hours").await).into_response()
}

async fn put_hours(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    if serde_json::from_value::<layers::HoursLayer>(value.clone()).is_err() {
        return fail(StatusCode::BAD_REQUEST, "invalid hours layer");
    }
    match write_layer(&state, &id, "hours", &value).await {
        Ok(()) => Json(value).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_rules(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(read_layer(&state, &id, "rules").await).into_response()
}

async fn put_rules(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    if serde_json::from_value::<Vec<Rule>>(value.clone()).is_err() {
        return fail(StatusCode::BAD_REQUEST, "invalid rules layer");
    }
    match write_layer(&state, &id, "rules", &value).await {
        Ok(()) => Json(value).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_items(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(read_layer(&state, &id, "items").await).into_response()
}

async fn put_items(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    if serde_json::from_value::<ItemsLayer>(value.clone()).is_err() {
        return fail(StatusCode::BAD_REQUEST, "invalid items layer");
    }
    match write_layer(&state, &id, "items", &value).await {
        Ok(()) => Json(value).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_prompt(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(read_layer(&state, &id, "prompt").await).into_response()
}

async fn put_prompt(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    let text = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let stored = Value::String(text);
    match write_layer(&state, &id, "prompt", &stored).await {
        Ok(()) => Json(stored).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

// ── rule scopes (layer 6) ────────────────────────────────────────────────────

async fn get_scopes(State(state): State<Arc<RoutesState>>) -> Response {
    Json(json!({ "scopes": read_scopes(state.storage.as_ref()).await })).into_response()
}

async fn add_scope(
    State(state): State<Arc<RoutesState>>,
    Json(value): Json<Value>,
) -> Response {
    let scope: RuleScope = match serde_json::from_value(value) {
        Ok(scope) => scope,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    scopes.push(scope.clone());
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => (StatusCode::CREATED, Json(json!({ "scope": scope }))).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn update_scope(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    let Some(index) = scopes.iter().position(|scope| scope.id == id) else {
        return fail(StatusCode::NOT_FOUND, "no such scope");
    };
    let mut scope: RuleScope = match serde_json::from_value(value) {
        Ok(scope) => scope,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    scope.id = id;
    scopes[index] = scope.clone();
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => Json(json!({ "scope": scope })).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn delete_scope(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
) -> Response {
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    scopes.retain(|scope| scope.id != id);
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

// ── compile (dry run) ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CompileQuery {
    date: Option<String>,
}

async fn compile(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Query(query): Query<CompileQuery>,
) -> Response {
    let date = query
        .date
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let Some(parsed) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok() else {
        return fail(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD");
    };
    let weekday = chrono::Datelike::weekday(&parsed).num_days_from_sunday();

    let channel = id.clone();
    let layers = ResolvedLayers {
        hours: serde_json::from_value(read_layer(&state, &channel, "hours").await)
            .unwrap_or_default(),
        rules: serde_json::from_value(read_layer(&state, &channel, "rules").await)
            .unwrap_or_default(),
        items: serde_json::from_value(read_layer(&state, &channel, "items").await)
            .unwrap_or_default(),
        prompt: read_layer(&state, &channel, "prompt")
            .await
            .as_str()
            .unwrap_or("")
            .to_string(),
        applied: Vec::new(),
    };
    let scopes = read_scopes(state.storage.as_ref()).await;
    let resolved = layers::resolve(layers, &scopes, &channel, &date, weekday, &[]);

    let settings = state.settings.lock().await.clone();
    let filter = catalog_filter(&resolved.items);
    let rows = match state.core.catalog(filter).await {
        Ok(rows) => rows,
        Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let pool: Vec<PoolItem> = rows
        .into_iter()
        .take(settings.pool_cap as usize)
        .map(|row| PoolItem {
            match_key: row.match_key,
            kind: row.kind,
            title: row.title,
            runtime_ms: row.runtime_ms,
        })
        .collect();

    let compiled = layers::compile(resolved, pool);
    Json(json!({
        "date": date,
        "channel": channel,
        "base_hash": compiled.base_hash,
        "prompt": compiled.prompt,
        "pool": compiled.pool,
        "layers": compiled.layers,
    }))
    .into_response()
}

/// Map the items layer onto the core's catalog filter. List ids are not
/// resolved yet (a later phase reads them from the Lists plugin).
fn catalog_filter(items: &ItemsLayer) -> CatalogFilter {
    let mut match_keys = items.match_keys.clone();
    for pin in &items.pins {
        if !pin.exclude {
            match_keys.push(pin.match_key.clone());
        }
    }
    CatalogFilter {
        kind: items.filter.kind.clone(),
        genres: items.filter.genres.clone(),
        tags: items.filter.tags.clone(),
        year_min: items.filter.year_min,
        year_max: items.filter.year_max,
        rating_max: items.filter.rating_max.clone(),
        match_keys,
        limit: None,
    }
}

// ── build status ─────────────────────────────────────────────────────────────

async fn build_status(State(state): State<Arc<RoutesState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(json!({
        "enabled": settings.enabled,
        "days_to_build": settings.days_to_build,
        "build_time": settings.build_time,
        "state": "idle",
        "detail": "the build loop lands in a later phase",
    }))
    .into_response()
}
