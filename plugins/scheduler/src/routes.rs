//! The Scheduler plugin's HTTP API: settings, the per-channel layers, the rule
//! scopes, the compile dry-run, one-day and whole-horizon builds, and status.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use channelflow_plugin_api::core::CoreData;
use channelflow_plugin_api::plugin::PluginLogger;
use channelflow_plugin_api::storage::PluginStorage;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::build;
use crate::horizon::Horizon;
use crate::layers::{self, ItemsLayer, PoolItem, Rule, RuleScope};
use crate::settings::{read_scopes, write_scopes, SchedulerSettings, SETTINGS_KEY};
use crate::state;

/// What the route handlers need.
pub struct RoutesState {
    pub storage: Arc<dyn PluginStorage>,
    pub core: Arc<dyn CoreData>,
    pub settings: Mutex<SchedulerSettings>,
    pub horizon: Arc<Horizon>,
    pub logger: PluginLogger,
}

pub fn router(state: Arc<RoutesState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/channels", get(list_channels))
        .route("/channels/{id}/hours", get(get_hours).put(put_hours))
        .route("/channels/{id}/rules", get(get_rules).put(put_rules))
        .route("/channels/{id}/items", get(get_items).put(put_items))
        .route("/channels/{id}/prompt", get(get_prompt).put(put_prompt))
        .route("/channels/{id}/compile", post(compile))
        .route("/channels/{id}/build", post(build_one))
        .route("/channels/{id}/days", get(list_days))
        .route("/channels/{id}/days/{date}", get(get_day).delete(delete_day))
        .route("/scopes", get(get_scopes).post(add_scope))
        .route("/scopes/{id}", put(update_scope).delete(delete_scope))
        .route("/build", post(build_horizon))
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

/// The channel list, from the core, so the page can pick one.
async fn list_channels(State(state): State<Arc<RoutesState>>) -> Response {
    match state.core.channels().await {
        Ok(channels) => Json(json!({ "channels": channels })).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
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

async fn get_hours(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(state::read_layer(state.storage.as_ref(), &id, "hours").await).into_response()
}

async fn put_hours(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    if serde_json::from_value::<layers::HoursLayer>(value.clone()).is_err() {
        return fail(StatusCode::BAD_REQUEST, "invalid hours layer");
    }
    match state::write_layer(state.storage.as_ref(), &id, "hours", &value).await {
        Ok(()) => Json(value).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_rules(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(state::read_layer(state.storage.as_ref(), &id, "rules").await).into_response()
}

async fn put_rules(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    if serde_json::from_value::<Vec<Rule>>(value.clone()).is_err() {
        return fail(StatusCode::BAD_REQUEST, "invalid rules layer");
    }
    match state::write_layer(state.storage.as_ref(), &id, "rules", &value).await {
        Ok(()) => Json(value).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_items(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(state::read_layer(state.storage.as_ref(), &id, "items").await).into_response()
}

async fn put_items(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    if serde_json::from_value::<ItemsLayer>(value.clone()).is_err() {
        return fail(StatusCode::BAD_REQUEST, "invalid items layer");
    }
    match state::write_layer(state.storage.as_ref(), &id, "items", &value).await {
        Ok(()) => Json(value).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_prompt(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(state::read_layer(state.storage.as_ref(), &id, "prompt").await).into_response()
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
    match state::write_layer(state.storage.as_ref(), &id, "prompt", &stored).await {
        Ok(()) => Json(stored).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

// ── rule scopes (layer 6) ────────────────────────────────────────────────────

async fn get_scopes(State(state): State<Arc<RoutesState>>) -> Response {
    Json(json!({ "scopes": read_scopes(state.storage.as_ref()).await })).into_response()
}

async fn add_scope(State(state): State<Arc<RoutesState>>, Json(value): Json<Value>) -> Response {
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

async fn delete_scope(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    scopes.retain(|scope| scope.id != id);
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

// ── compile (dry run) ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DateQuery {
    date: Option<String>,
}

async fn compile(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Query(query): Query<DateQuery>,
) -> Response {
    let date = query
        .date
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let Some(parsed) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok() else {
        return fail(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD");
    };
    let weekday = chrono::Datelike::weekday(&parsed).num_days_from_sunday();

    let layers = state::read_channel_layers(state.storage.as_ref(), &id).await;
    let scopes = read_scopes(state.storage.as_ref()).await;
    let resolved = layers::resolve(layers, &scopes, &id, &date, weekday, &[]);

    let settings = state.settings.lock().await.clone();
    let rows = match state
        .core
        .catalog(build::catalog_filter(&resolved.items))
        .await
    {
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
        "channel": id,
        "base_hash": compiled.base_hash,
        "prompt": compiled.prompt,
        "pool": compiled.pool,
        "layers": compiled.layers,
    }))
    .into_response()
}

// ── one day, and the stored days ─────────────────────────────────────────────

async fn build_one(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Query(query): Query<DateQuery>,
) -> Response {
    let date = query
        .date
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let Some(parsed) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok() else {
        return fail(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD");
    };
    let weekday = chrono::Datelike::weekday(&parsed).num_days_from_sunday();

    let layers = state::read_channel_layers(state.storage.as_ref(), &id).await;
    let scopes = read_scopes(state.storage.as_ref()).await;
    let resolved = layers::resolve(layers, &scopes, &id, &date, weekday, &[]);
    let settings = state.settings.lock().await.clone();
    let previous = state::read_day(state.storage.as_ref(), &id, &date).await;

    let outcome = match build::build_day(
        &state.core,
        &id,
        &date,
        resolved,
        &settings,
        previous.as_ref(),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    };
    if !outcome.skipped {
        if let Err(error) = state::store_day(state.storage.as_ref(), &id, &outcome.day).await {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    Json(json!({ "outcome": outcome })).into_response()
}

async fn list_days(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    let index = state::read_days_index(state.storage.as_ref(), &id).await;
    let mut days = Vec::new();
    for date in index {
        if let Some(day) = state::read_day(state.storage.as_ref(), &id, &date).await {
            days.push(json!({
                "date": day.date,
                "degraded": day.degraded,
                "blocks": day.blocks.len(),
                "provider": day.provider,
                "created_at": day.created_at,
            }));
        }
    }
    Json(json!({ "days": days })).into_response()
}

async fn get_day(
    State(state): State<Arc<RoutesState>>,
    Path((id, date)): Path<(String, String)>,
) -> Response {
    match state::read_day(state.storage.as_ref(), &id, &date).await {
        Some(day) => Json(json!({ "day": day })).into_response(),
        None => fail(StatusCode::NOT_FOUND, "no day built for that date"),
    }
}

async fn delete_day(
    State(state): State<Arc<RoutesState>>,
    Path((id, date)): Path<(String, String)>,
) -> Response {
    match state::delete_day(state.storage.as_ref(), &id, &date).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

// ── the whole horizon ────────────────────────────────────────────────────────

async fn build_horizon(State(state): State<Arc<RoutesState>>) -> Response {
    if state.horizon.is_running() {
        return (
            StatusCode::ACCEPTED,
            Json(json!({ "status": "already running" })),
        )
            .into_response();
    }
    let horizon = state.horizon.clone();
    let core = state.core.clone();
    let storage = state.storage.clone();
    let settings = state.settings.lock().await.clone();
    let logger = state.logger.clone();
    tokio::spawn(async move {
        horizon
            .run(&core, &storage, settings, &logger, "manual")
            .await;
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "started" })),
    )
        .into_response()
}

async fn build_status(State(state): State<Arc<RoutesState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let progress = state.horizon.snapshot().await;
    Json(json!({ "settings": settings, "progress": progress })).into_response()
}
