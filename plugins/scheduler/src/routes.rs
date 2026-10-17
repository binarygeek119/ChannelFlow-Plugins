//! The Scheduler plugin's HTTP API: settings, the per-channel layers, the rule
//! scopes, the compile dry-run, one-day and whole-horizon builds, and status.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use channelflow_plugin_api::core::CoreData;
use channelflow_plugin_api::plugin::PluginLogger;
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::tasks::{TaskGuard, TaskHost, TaskInfo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::build;
use crate::horizon::Horizon;
use crate::layers::{self, ItemsLayer, Rule, RuleScope};
use crate::settings::{read_scopes, write_scopes, SchedulerSettings, SETTINGS_KEY};
use crate::state;

/// What the route handlers need.
pub struct RoutesState {
    pub core: Arc<dyn CoreData>,
    pub storage: Arc<dyn PluginStorage>,
    pub settings: Mutex<SchedulerSettings>,
    pub horizon: Arc<Horizon>,
    /// Where a one-day build reports itself, so the base's status popup shows
    /// it from any page.
    pub tasks: Arc<dyn TaskHost>,
    /// `<config>`, where the playout folder lives.
    pub config_root: PathBuf,
    pub logger: PluginLogger,
    /// True while a whole-catalog playout rebuild runs, so a second trigger is
    /// refused rather than racing the first.
    pub rebuilding: AtomicBool,
}

pub fn router(state: Arc<RoutesState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/channels", get(list_channels))
        .route("/channels/{id}/hours", get(get_hours).put(put_hours))
        .route("/channels/{id}/rules", get(get_rules).put(put_rules))
        .route("/channels/{id}/items", get(get_items).put(put_items))
        .route("/channels/{id}/prompt", get(get_prompt).put(put_prompt))
        .route("/channels/{id}/layers", post(put_layers))
        .route("/channels/{id}/compile", post(compile))
        .route("/channels/{id}/build", post(build_one))
        .route("/channels/{id}/days", get(list_days))
        .route("/channels/{id}/days/{date}", get(get_day).delete(delete_day))
        .route("/channels/{id}/playout", post(write_playout))
        .route("/scopes", get(get_scopes).post(add_scope))
        .route("/scopes/{id}", put(update_scope).delete(delete_scope))
        .route("/build", post(build_horizon))
        .route("/build/status", get(build_status))
        .route("/build/cancel", post(cancel_build))
        .route("/channels/{id}/clear", post(clear_channel))
        .route("/guide/clear", post(clear_guide))
        .route("/playout/rebuild", post(rebuild_playouts))
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
    let old_settings = state.settings.lock().await.clone();
    let settings = match SchedulerSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => {
            state.logger.warn(&format!("scheduler: invalid settings update: {}", error));
            return fail(StatusCode::BAD_REQUEST, error);
        }
    };
    
    // Log important setting changes
    if old_settings.enabled != settings.enabled {
        state.logger.info(&format!(
            "scheduler: enabled changed from {} to {}",
            old_settings.enabled, settings.enabled
        ));
    }
    if old_settings.build_time != settings.build_time {
        state.logger.info(&format!(
            "scheduler: build_time changed from {} to {}",
            old_settings.build_time, settings.build_time
        ));
    }
    if old_settings.write_playout != settings.write_playout {
        state.logger.info(&format!(
            "scheduler: write_playout changed from {} to {} (this {} playout files being written)",
            old_settings.write_playout, 
            settings.write_playout,
            if settings.write_playout { "enables" } else { "disables" }
        ));
    }
    if old_settings.days_to_build != settings.days_to_build {
        state.logger.info(&format!(
            "scheduler: days_to_build changed from {} to {}",
            old_settings.days_to_build, settings.days_to_build
        ));
    }
    
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        state.logger.error(&format!("scheduler: failed to save settings: {}", error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("scheduler: settings updated successfully");
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
    state.logger.info(&format!("scheduler: updating hours layer for channel {}", id));
    if serde_json::from_value::<layers::HoursLayer>(value.clone()).is_err() {
        state.logger.warn(&format!("scheduler: invalid hours layer for channel {}", id));
        return fail(StatusCode::BAD_REQUEST, "invalid hours layer");
    }
    match state::write_layer(state.storage.as_ref(), &id, "hours", &value).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: hours layer updated for channel {}", id));
            Json(value).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to update hours layer for channel {}: {}", id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
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
    state.logger.info(&format!("scheduler: updating rules layer for channel {}", id));
    if serde_json::from_value::<Vec<Rule>>(value.clone()).is_err() {
        state.logger.warn(&format!("scheduler: invalid rules layer for channel {}", id));
        return fail(StatusCode::BAD_REQUEST, "invalid rules layer");
    }
    match state::write_layer(state.storage.as_ref(), &id, "rules", &value).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: rules layer updated for channel {}", id));
            Json(value).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to update rules layer for channel {}: {}", id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
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
    state.logger.info(&format!("scheduler: updating items layer for channel {}", id));
    if serde_json::from_value::<ItemsLayer>(value.clone()).is_err() {
        state.logger.warn(&format!("scheduler: invalid items layer for channel {}", id));
        return fail(StatusCode::BAD_REQUEST, "invalid items layer");
    }
    match state::write_layer(state.storage.as_ref(), &id, "items", &value).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: items layer updated for channel {}", id));
            Json(value).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to update items layer for channel {}: {}", id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
    }
}

/// The four layer documents in one call — what a preset applies after it
/// creates a channel.
#[derive(Debug, Deserialize)]
struct LayersBundle {
    #[serde(default)]
    hours: layers::HoursLayer,
    #[serde(default)]
    rules: Vec<Rule>,
    #[serde(default)]
    items: ItemsLayer,
    #[serde(default)]
    prompt: String,
}

async fn put_layers(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(bundle): Json<LayersBundle>,
) -> Response {
    state.logger.info(&format!("scheduler: updating all layers for channel {}", id));
    
    let hours = serde_json::to_value(&bundle.hours).unwrap_or(Value::Null);
    let rules = serde_json::to_value(&bundle.rules).unwrap_or(Value::Null);
    let items = serde_json::to_value(&bundle.items).unwrap_or(Value::Null);
    let prompt = Value::String(bundle.prompt);
    let pairs: [(&str, &Value); 4] = [
        ("hours", &hours),
        ("rules", &rules),
        ("items", &items),
        ("prompt", &prompt),
    ];
    for (layer, value) in pairs {
        if let Err(error) = state::write_layer(state.storage.as_ref(), &id, layer, value).await {
            state.logger.error(&format!(
                "scheduler: failed to update {} layer for channel {}: {}",
                layer, id, error
            ));
            return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    
    state.logger.info(&format!("scheduler: all layers updated for channel {}", id));
    Json(json!({ "hours": hours, "rules": rules, "items": items, "prompt": prompt })).into_response()
}

async fn get_prompt(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    Json(state::read_layer(state.storage.as_ref(), &id, "prompt").await).into_response()
}

async fn put_prompt(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    state.logger.info(&format!("scheduler: updating prompt layer for channel {}", id));
    
    let text = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let stored = Value::String(text);
    
    match state::write_layer(state.storage.as_ref(), &id, "prompt", &stored).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: prompt layer updated for channel {}", id));
            Json(stored).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to update prompt layer for channel {}: {}", id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
    }
}

// ── rule scopes (layer 6) ────────────────────────────────────────────────────

async fn get_scopes(State(state): State<Arc<RoutesState>>) -> Response {
    Json(json!({ "scopes": read_scopes(state.storage.as_ref()).await })).into_response()
}

async fn add_scope(State(state): State<Arc<RoutesState>>, Json(value): Json<Value>) -> Response {
    state.logger.info("scheduler: adding new rule scope");
    
    let scope: RuleScope = match serde_json::from_value(value) {
        Ok(scope) => scope,
        Err(error) => {
            state.logger.warn(&format!("scheduler: invalid scope data: {}", error));
            return fail(StatusCode::BAD_REQUEST, error.to_string());
        }
    };
    
    state.logger.info(&format!("scheduler: new scope with id {}", scope.id));
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    scopes.push(scope.clone());
    
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: scope {} added successfully ({} total scopes)", scope.id, scopes.len()));
            (StatusCode::CREATED, Json(json!({ "scope": scope }))).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to add scope {}: {}", scope.id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
    }
}

async fn update_scope(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Response {
    state.logger.info(&format!("scheduler: updating scope {}", id));
    
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    let Some(index) = scopes.iter().position(|scope| scope.id == id) else {
        state.logger.warn(&format!("scheduler: scope {} not found", id));
        return fail(StatusCode::NOT_FOUND, "no such scope");
    };
    
    let mut scope: RuleScope = match serde_json::from_value(value) {
        Ok(scope) => scope,
        Err(error) => {
            state.logger.warn(&format!("scheduler: invalid scope data for {}: {}", id, error));
            return fail(StatusCode::BAD_REQUEST, error.to_string());
        }
    };
    scope.id = id;
    scopes[index] = scope.clone();
    
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: scope {} updated successfully", scope.id));
            Json(json!({ "scope": scope })).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to update scope {}: {}", scope.id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
    }
}

async fn delete_scope(State(state): State<Arc<RoutesState>>, Path(id): Path<String>) -> Response {
    state.logger.info(&format!("scheduler: deleting scope {}", id));
    
    let mut scopes = read_scopes(state.storage.as_ref()).await;
    let initial_count = scopes.len();
    scopes.retain(|scope| scope.id != id);
    
    if scopes.len() == initial_count {
        state.logger.warn(&format!("scheduler: scope {} not found for deletion", id));
    }
    
    match write_scopes(state.storage.as_ref(), &scopes).await {
        Ok(()) => {
            state.logger.info(&format!("scheduler: scope {} deleted ({} scopes remaining)", id, scopes.len()));
            StatusCode::NO_CONTENT.into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: failed to delete scope {}: {}", id, error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
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
    
    state.logger.info(&format!("scheduler: compile dry run for channel {} on {}", id, date));
    
    let Some(parsed) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok() else {
        state.logger.warn(&format!("scheduler: invalid date format for compile: {}", date));
        return fail(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD");
    };
    let weekday = chrono::Datelike::weekday(&parsed).num_days_from_sunday();

    let layers = state::read_channel_layers(state.storage.as_ref(), &id).await;
    let scopes = read_scopes(state.storage.as_ref()).await;
    let resolved = layers::resolve(layers, &scopes, &id, &date, weekday, &[]);
    
    state.logger.info(&format!(
        "scheduler: compile for {} {} resolved with {} rules and {} applied scopes",
        id, date, resolved.rules.len(), resolved.applied.len()
    ));

    let settings = state.settings.lock().await.clone();
    let rows = match state
        .core
        .catalog(build::catalog_filter(&resolved.items))
        .await
    {
        Ok(rows) => rows,
        Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let pool = build::interleave(rows, &resolved.items.filter.kinds, settings.pool_cap as usize);

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
    state.logger.info(&format!("scheduler: manual build requested for {} {}", id, date));
    
    let Some(parsed) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok() else {
        state.logger.warn(&format!("scheduler: invalid date format for build: {}", date));
        return fail(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD");
    };
    let weekday = chrono::Datelike::weekday(&parsed).num_days_from_sunday();

    let layers = state::read_channel_layers(state.storage.as_ref(), &id).await;
    let scopes = read_scopes(state.storage.as_ref()).await;
    let resolved = layers::resolve(layers, &scopes, &id, &date, weekday, &[]);
    let settings = state.settings.lock().await.clone();
    let settings_for_write = settings.clone();
    let previous = state::read_day(state.storage.as_ref(), &id, &date).await;
    
    state.logger.info(&format!(
        "scheduler: building {} {} with {} rules, {} scopes applied, write_playout={}",
        id, date, resolved.rules.len(), resolved.applied.len(), settings_for_write.write_playout
    ));

    // One day is one model call, which can take a while: report it so the
    // base's status popup shows it from any page until the guard drops.
    let _task = TaskGuard::start(
        state.tasks.clone(),
        TaskInfo {
            key: "build-day".to_string(),
            title: format!("Scheduler build · {id}"),
            label: Some(date.clone()),
            ..Default::default()
        },
    );

    let horizon = state.horizon.clone();
    let core = state.core.clone();
    let storage = state.storage.clone();
    let channel = id.clone();
    let build_date = date.clone();
    let logger = state.logger.clone();
    let handle = tokio::spawn(async move {
        build::build_day(&core, &channel, &build_date, resolved, &settings, previous.as_ref(), &logger).await
    });
    horizon.set_abort(handle.abort_handle());
    let outcome = match handle.await {
        Ok(Ok(outcome)) => {
            if outcome.skipped {
                state.logger.info(&format!("scheduler: {} {} build skipped (inputs unchanged)", id, date));
            } else {
                state.logger.info(&format!(
                    "scheduler: {} {} built successfully ({} blocks, degraded: {})",
                    id, date, outcome.day.blocks.len(), outcome.day.degraded
                ));
            }
            outcome
        },
        Ok(Err(error)) => {
            state.logger.error(&format!("scheduler: {} {} build failed: {}", id, date, error));
            return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
        },
        Err(_) => {
            state.logger.warn(&format!("scheduler: {} {} build cancelled", id, date));
            return (
                StatusCode::CONFLICT,
                Json(json!({ "status": "cancelled" })),
            )
                .into_response()
        }
    };
    if !outcome.skipped {
        if let Err(error) = state::store_day(storage.as_ref(), &id, &outcome.day).await {
            state.logger.error(&format!("scheduler: {} {} failed to store day: {}", id, date, error));
            return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
        state.logger.info(&format!("scheduler: {} {} day stored", id, date));
    }
    // Always write the guide and lineup, regardless of write_playout setting.
    // The guide and lineup should be populated when building a day.
    if let Err(error) = crate::playout::set_guide(&state.core, &outcome.day, &state.logger).await {
        state.logger.error(&format!("scheduler: {} {} failed to set guide: {}", id, date, error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
    }
    if let Err(error) = crate::playout::set_lineup(&state.core, &outcome.day, &state.logger).await {
        state.logger.error(&format!("scheduler: {} {} failed to set lineup: {}", id, date, error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
    }
    // Only write playout files if write_playout is enabled.
    if settings_for_write.write_playout {
        let dir = crate::playout::channel_dir(
            &state.config_root,
            &settings_for_write.playout_dir,
            &id,
        );
        if let Err(error) = crate::playout::write(&state.core, &outcome.day, &dir, &state.logger).await {
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

async fn write_playout(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
    Query(query): Query<DateQuery>,
) -> Response {
    let date = query
        .date
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let Some(day) = state::read_day(state.storage.as_ref(), &id, &date).await else {
        state.logger.warn(&format!("scheduler: write_playout called for {} {} but no day found", id, date));
        return fail(StatusCode::NOT_FOUND, "no day built for that date");
    };
    state.logger.info(&format!("scheduler: write_playout called for {} {}", id, date));
    let settings = state.settings.lock().await.clone();
    let dir = crate::playout::channel_dir(&state.config_root, &settings.playout_dir, &id);
    // Always set the guide and lineup before writing the playout file
    if let Err(error) = crate::playout::set_guide(&state.core, &day, &state.logger).await {
        state.logger.error(&format!("scheduler: write_playout failed to set guide: {}", error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
    }
    if let Err(error) = crate::playout::set_lineup(&state.core, &day, &state.logger).await {
        state.logger.error(&format!("scheduler: write_playout failed to set lineup: {}", error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
    }
    match crate::playout::write(&state.core, &day, &dir, &state.logger).await {
        Ok(report) => {
            state.logger.info(&format!("scheduler: write_playout completed successfully for {} {}", id, date));
            Json(json!({ "report": report })).into_response()
        },
        Err(error) => {
            state.logger.error(&format!("scheduler: write_playout failed: {}", error));
            fail(StatusCode::INTERNAL_SERVER_ERROR, error)
        },
    }
}

/// Clears the rebuild flag however the handler returns, so a failed write or an
/// early return cannot leave the plugin refusing every later rebuild.
struct RebuildFlag<'a>(&'a AtomicBool);

impl Drop for RebuildFlag<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// What a whole-catalog playout rebuild did.
#[derive(Debug, Clone, Serialize)]
pub struct RebuildReport {
    pub channels: usize,
    pub days: usize,
    pub items: usize,
    pub missing: usize,
    pub errors: Vec<String>,
}

/// Rewrite every stored day's playout JSON for every channel that has days, and
/// reset each channel's guide from what is stored. No model calls — the days
/// themselves are untouched — so this is the recovery step after a file move or
/// a cleared guide. Shared by the route and the tests.
pub async fn rebuild_all_playouts(
    core: &Arc<dyn CoreData>,
    storage: &Arc<dyn PluginStorage>,
    config_root: &std::path::Path,
    settings: &SchedulerSettings,
    task: &TaskGuard,
    logger: &channelflow_plugin_api::plugin::PluginLogger,
) -> RebuildReport {
    logger.info("scheduler: starting rebuild of all playouts");
    
    let mut report = RebuildReport {
        channels: 0,
        days: 0,
        items: 0,
        missing: 0,
        errors: Vec::new(),
    };
    
    let channels = match core.channels().await {
        Ok(channels) => {
            logger.info(&format!("scheduler: found {} channels to rebuild", channels.len()));
            channels
        },
        Err(error) => {
            logger.error(&format!("scheduler: failed to fetch channels: {}", error));
            report.errors.push(error.to_string());
            return report;
        }
    };
    
    for channel in &channels {
        let dates = state::read_days_index(storage.as_ref(), &channel.id).await;
        if dates.is_empty() {
            logger.info(&format!("scheduler: {} has no days to rebuild, skipping", channel.id));
            continue;
        }
        
        logger.info(&format!("scheduler: rebuilding {} days for channel {}", dates.len(), channel.id));
        
        let dir = crate::playout::channel_dir(config_root, &settings.playout_dir, &channel.id);
        let mut wrote_any = false;
        
        for date in &dates {
            let Some(day) = state::read_day(storage.as_ref(), &channel.id, date).await else {
                logger.warn(&format!("scheduler: {} {} day not found in storage", channel.id, date));
                continue;
            };
            
            task.progress(
                &format!("{} · {date}", channel.name),
                Some(report.days as u64),
                None,
            );
            
            // Always set the guide and lineup before writing the playout file
            if let Err(error) = crate::playout::set_guide(core, &day, logger).await {
                logger.error(&format!("scheduler: {} {} failed to set guide: {}", channel.id, date, error));
                report.errors.push(format!("{} {} guide: {}", channel.id, date, error));
                continue;
            }
            
            if let Err(error) = crate::playout::set_lineup(core, &day, logger).await {
                logger.error(&format!("scheduler: {} {} failed to set lineup: {}", channel.id, date, error));
                report.errors.push(format!("{} {} lineup: {}", channel.id, date, error));
                continue;
            }
            
            match crate::playout::write(core, &day, &dir, logger).await {
                Ok(written) => {
                    logger.info(&format!("scheduler: {} {} playout rebuilt ({} items, {} missing)", 
                        channel.id, date, written.items, written.missing));
                    report.days += 1;
                    report.items += written.items;
                    report.missing += written.missing;
                    wrote_any = true;
                }
                Err(error) => {
                    logger.error(&format!("scheduler: {} {} failed to write playout: {}", channel.id, date, error));
                    report.errors.push(format!("{} · {date}: {error}", channel.name));
                }
            }
        }
        
        if wrote_any {
            report.channels += 1;
        }
    }
    
    logger.info(&format!(
        "scheduler: rebuild complete - {} channels, {} days, {} items, {} missing, {} errors",
        report.channels, report.days, report.items, report.missing, report.errors.len()
    ));
    
    report
}

/// "Rebuild All Playouts": the base Tasks page calls it and then refreshes the
/// encoder's `lineup.json`/`channel.json` (its `/api/playout/regenerate`).
async fn rebuild_playouts(State(state): State<Arc<RoutesState>>) -> Response {
    state.logger.info("scheduler: rebuild all playouts requested");
    
    if state.rebuilding.swap(true, Ordering::SeqCst) {
        state.logger.warn("scheduler: rebuild all playouts rejected - already running");
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "a playout rebuild is already running" })),
        )
            .into_response();
    }
    let _reset = RebuildFlag(&state.rebuilding);

    let settings = state.settings.lock().await.clone();
    let task = TaskGuard::start(
        state.tasks.clone(),
        TaskInfo {
            key: "rebuild-playouts".to_string(),
            title: "Rebuild all playouts".to_string(),
            ..Default::default()
        },
    );

    let report = rebuild_all_playouts(
        &state.core,
        &state.storage,
        &state.config_root,
        &settings,
        &task,
        &state.logger,
    )
    .await;

    state.logger.info(&format!(
        "scheduler: rebuild all playouts complete - {} channels, {} days, {} errors",
        report.channels, report.days, report.errors.len()
    ));

    Json(json!({ "report": report })).into_response()
}

async fn build_horizon(State(state): State<Arc<RoutesState>>) -> Response {
    state.logger.info("scheduler: manual horizon build requested");
    
    if state.horizon.is_running() {
        state.logger.warn("scheduler: horizon build rejected - already running");
        return (
            StatusCode::ACCEPTED,
            Json(json!({ "status": "already running" })),
        )
            .into_response();
    }
    
    state.logger.info("scheduler: starting horizon build");
    let horizon = state.horizon.clone();
    let core = state.core.clone();
    let storage = state.storage.clone();
    let config_root = state.config_root.clone();
    let settings = state.settings.lock().await.clone();
    let logger = state.logger.clone();
    let task_horizon = horizon.clone();
    let handle = tokio::spawn(async move {
        task_horizon
            .run(&core, &storage, &config_root, settings, &logger, "manual")
            .await;
    });
    horizon.set_abort(handle.abort_handle());
    
    state.logger.info("scheduler: horizon build task spawned");
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "started" })),
    )
        .into_response()
}

async fn cancel_build(State(state): State<Arc<RoutesState>>) -> Response {
    state.logger.info("scheduler: build cancellation requested");
    state.horizon.cancel();
    Json(json!({ "status": "cancelling" })).into_response()
}

/// Clear everything the scheduler produced for one channel: its days, index,
/// history, and the channel's guide programmes.
async fn clear_channel(
    State(state): State<Arc<RoutesState>>,
    Path(id): Path<String>,
) -> Response {
    state.logger.info(&format!("scheduler: clearing channel {}", id));
    
    let storage = state.storage.as_ref();
    let index = state::read_days_index(storage, &id).await;
    let days_count = index.len();
    
    for date in &index {
        let _ = state::delete_day(storage, &id, date).await;
    }
    let _ = storage.delete(&state::days_index_key(&id)).await;
    let _ = storage.delete(&state::history_key(&id)).await;
    let _ = state.core.set_channel_guide(&id, Vec::new()).await;
    
    state.logger.info(&format!(
        "scheduler: channel {} cleared ({} days removed, guide cleared)",
        id, days_count
    ));
    
    Json(json!({ "cleared_days": index.len() })).into_response()
}

/// Clear every channel's guide programmes.
async fn clear_guide(State(state): State<Arc<RoutesState>>) -> Response {
    state.logger.info("scheduler: clearing all channel guides");
    
    let channels = state.core.channels().await.unwrap_or_default();
    state.logger.info(&format!("scheduler: found {} channels to clear", channels.len()));
    
    let mut cleared = 0usize;
    for channel in &channels {
        if state
            .core
            .set_channel_guide(&channel.id, Vec::new())
            .await
            .is_ok()
        {
            cleared += 1;
        }
    }
    
    state.logger.info(&format!(
        "scheduler: cleared guides for {} of {} channels",
        cleared,
        channels.len()
    ));
    
    Json(json!({ "cleared_channels": cleared })).into_response()
}

async fn build_status(State(state): State<Arc<RoutesState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let progress = state.horizon.snapshot().await;
    Json(json!({ "settings": settings, "progress": progress })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelflow_plugin_api::core::{
        CatalogFilter, CatalogRow, ChatRequest, ChatResponse, CoreChannel, CoreDataError,
    };
    use channelflow_plugin_api::storage::PluginStorageError;
    use channelflow_plugin_api::NoopTaskHost;

    use crate::build::{AiredBlock, AiredItem, GeneratedDay};
    use std::collections::HashMap;

    /// A real in-memory store (the SDK's `InMemoryStorage` is read-only).
    #[derive(Default)]
    struct MemStorage {
        inner: std::sync::Mutex<HashMap<String, Value>>,
    }

    #[async_trait::async_trait]
    impl PluginStorage for MemStorage {
        async fn get(&self, key: &str) -> Result<Option<Value>, PluginStorageError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &Value) -> Result<(), PluginStorageError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.clone());
            Ok(())
        }
        async fn delete(&self, key: &str) -> Result<(), PluginStorageError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct FakeCore {
        channels: Vec<CoreChannel>,
    }

    #[async_trait::async_trait]
    impl CoreData for FakeCore {
        async fn channels(&self) -> Result<Vec<CoreChannel>, CoreDataError> {
            Ok(self.channels.clone())
        }
        async fn installed_plugins(&self) -> Result<Vec<String>, CoreDataError> {
            Ok(Vec::new())
        }
        async fn catalog(&self, _filter: CatalogFilter) -> Result<Vec<CatalogRow>, CoreDataError> {
            Ok(Vec::new())
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, CoreDataError> {
            Err(CoreDataError("unused".into()))
        }
    }

    fn a_day(channel: &str, date: &str) -> GeneratedDay {
        GeneratedDay {
            channel_id: channel.into(),
            date: date.into(),
            base_hash: "b".into(),
            input_hash: "i".into(),
            blocks: vec![AiredBlock {
                slot_index: 0,
                span_slots: 4,
                start_ms: 6 * 60 * 60 * 1000,
                finish_ms: 7 * 60 * 60 * 1000,
                filled_ms: 30 * 60 * 1000,
                items: vec![AiredItem {
                    match_key: "movie:x".into(),
                    title: "X".into(),
                    runtime_ms: Some(30 * 60 * 1000),
                    why: "test".into(),
                }],
            }],
            degraded: false,
            provider: None,
            created_at: "now".into(),
            validation: vec![],
            prompt: None,
        }
    }

    #[tokio::test]
    async fn rebuilding_writes_every_stored_day_and_skips_channels_without_days() {
        let core: Arc<dyn CoreData> = Arc::new(FakeCore {
            channels: vec![
                CoreChannel { id: "c1".into(), number: 1.0, name: "One".into(), enabled: true },
                CoreChannel { id: "c2".into(), number: 2.0, name: "Two".into(), enabled: true },
            ],
        });
        let storage: Arc<dyn PluginStorage> = Arc::new(MemStorage::default());
        state::store_day(storage.as_ref(), "c1", &a_day("c1", "2026-10-16"))
            .await
            .expect("store day 1");
        state::store_day(storage.as_ref(), "c1", &a_day("c1", "2026-10-17"))
            .await
            .expect("store day 2");
        // c2 has no days: it is skipped, not counted.

        let root = std::env::temp_dir().join(format!("channelflow-rebuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let settings = SchedulerSettings {
            playout_dir: root.join("playout").to_string_lossy().to_string(),
            ..Default::default()
        };
        let task = TaskGuard::start(
            Arc::new(NoopTaskHost),
            TaskInfo { key: "test".into(), title: "test".into(), ..Default::default() },
        );
        let logger = channelflow_plugin_api::plugin::PluginLogger::new("com.channelflow.scheduler");

        let report = rebuild_all_playouts(&core, &storage, &root, &settings, &task, &logger).await;
        assert_eq!(report.channels, 1, "only the channel that has days");
        assert_eq!(report.days, 2);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let files = std::fs::read_dir(root.join("playout").join("c1"))
            .expect("c1 playout folder")
            .filter_map(|entry| entry.ok())
            .count();
        assert_eq!(files, 2, "one playout file per stored day");
        let _ = std::fs::remove_dir_all(&root);
    }
}
