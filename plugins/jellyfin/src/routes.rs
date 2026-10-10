//! The plugin's API routes, mounted by the base under
//! `/api/plugins/com.channelflow.jellyfin`. The connection object travels in
//! the request body — the base stores the connections, the plugin never
//! persists keys of its own.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use channelflow_plugin_api::database::PluginDatabase;
use channelflow_plugin_api::media::{Connection, Library, SyncCtx};
use serde::Deserialize;
use serde_json::json;

use crate::client::JellyfinClient;
use crate::db::MediaDb;
use crate::selection::{self, Candidate, PlaybackPrefs};
use crate::sync;

pub struct RouteState {
    pub db: Arc<dyn PluginDatabase>,
}

pub fn router(db: Arc<dyn PluginDatabase>) -> Router {
    Router::new()
        .route("/libraries", post(libraries_request))
        .route("/sync", post(sync_request))
        .route("/progress", get(progress_snapshot))
        .route("/people", get(people_list))
        .route("/people/{name}", get(person_detail))
        .route("/item/{jellyfin_id}/detail", get(item_detail))
        .route(
            "/items/{id}/sources",
            get(item_sources).post(item_sources),
        )
        .route("/items/{id}/pin", post(pin_file))
        .route("/health-events", get(health_events))
        .route("/merge", get(merge_screen))
        .route("/merge-candidates", get(merge_candidates))
        .route("/merge", post(merge_request))
        .route("/split", post(split_request))
        .with_state(Arc::new(RouteState { db }))
}

/// The manual merge/split screen.
async fn merge_screen() -> Html<&'static str> {
    Html(include_str!("../static/merge.html"))
}

/// Every group one operation-shy of being wrong: colliding dedup keys, or a
/// single row holding several sources.
async fn merge_candidates(State(state): State<Arc<RouteState>>) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.merge_candidates().await {
        Ok(candidates) => Json(json!({ "candidates": candidates })).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

#[derive(Deserialize)]
struct MergeRequest {
    from_id: i64,
    to_id: i64,
}

/// Move `from`'s sources (and files) into `to`; `from` becomes hidden history.
async fn merge_request(
    State(state): State<Arc<RouteState>>,
    Json(input): Json<MergeRequest>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.merge_items(input.from_id, input.to_id).await {
        Ok(()) => Json(json!({ "merged": { "from": input.from_id, "into": input.to_id } }))
            .into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

#[derive(Deserialize)]
struct SplitRequest {
    item_id: i64,
}

/// Break a multi-source row into one row per source.
async fn split_request(
    State(state): State<Arc<RouteState>>,
    Json(input): Json<SplitRequest>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.split_item(input.item_id).await {
        Ok(created) => {
            Json(json!({ "item_id": input.item_id, "created": created })).into_response()
        }
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

#[derive(Deserialize)]
struct ConnectionRequest {
    connection: Connection,
    api_key: String,
}

/// List the libraries a connection can sync.
async fn libraries_request(Json(input): Json<ConnectionRequest>) -> Response {
    let client = match JellyfinClient::new(&input.connection, &input.api_key) {
        Ok(client) => client,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.0),
    };
    match client.libraries().await {
        Ok(libraries) => Json(json!({ "libraries": libraries })).into_response(),
        Err(error) => fail(StatusCode::BAD_GATEWAY, format!("libraries: {}", error.0)),
    }
}

#[derive(Deserialize)]
struct SyncRequest {
    connection_id: i64,
    connection: Connection,
    api_key: String,
    #[serde(default)]
    libraries: Vec<Library>,
    /// The core's image root; posters land under `<root>/posters/…`.
    #[serde(default)]
    image_root: Option<String>,
}

/// The live sync progress snapshot the popup polls while a sync runs.
async fn progress_snapshot() -> Json<serde_json::Value> {
    let state = crate::sync::progress_state().lock().unwrap();
    Json(serde_json::json!({ "progress": state.clone() }))
}

/// All synced people, alphabetical by name.
async fn people_list(State(state): State<Arc<RouteState>>) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.people_list().await {
        Ok(people) => Json(json!({ "people": people })).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

/// One person and every catalog item they appear in.
async fn person_detail(
    State(state): State<Arc<RouteState>>,
    Path(name): Path<String>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.person_filmography(&name).await {
        Ok(Some(person)) => Json(json!({ "person": person })).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such person" })))
            .into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

/// The rich metadata behind one Media-page item, by its Jellyfin id.
async fn item_detail(
    State(state): State<Arc<RouteState>>,
    Path(jellyfin_id): Path<String>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.get_item_detail(&jellyfin_id).await {
        Ok(Some(item)) => Json(json!({ "item": item })).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such item" })))
            .into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

/// Run one sync pass against the listed libraries.
async fn sync_request(
    State(state): State<Arc<RouteState>>,
    Json(input): Json<SyncRequest>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    if let Err(error) = media.init().await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.0);
    }
    let image_root = PathBuf::from(input.image_root.unwrap_or_default());
    let ctx = SyncCtx {
        connection: input.connection,
        api_key: input.api_key,
        enabled_libraries: input.libraries,
        connection_id: input.connection_id,
        db: state.db.clone(),
        image_root,
        remaps: serde_json::Value::Null,
        catalog: None, // plugin-hosted runs have no base catalog handle
    };
    let report = sync::run(&ctx, state.db.clone()).await;
    let _ = media
        .record_sync_run(
            input.connection_id,
            "ok",
            report.added,
            report.updated,
            report.removed,
            report.errors,
            "",
        )
        .await;
    Json(json!({ "report": report })).into_response()
}

/// Every file for an item, with the picker's winner marked.
async fn item_sources(
    State(state): State<Arc<RouteState>>,
    Path(id): Path<i64>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    let rows = match media.item_files(id).await {
        Ok(rows) => rows,
        Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    };
    let candidates: Vec<Candidate> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| Candidate {
            file_id: row["file_id"].as_i64().unwrap_or(index as i64),
            source_id: row["source_id"].as_i64().unwrap_or(0),
            connection_id: row["connection_id"].as_i64().unwrap_or(0),
            height: row["height"].as_i64().map(|height| height as i32),
            video_codec: row["video_codec"].as_str().map(str::to_string),
            hdr_format: row["hdr_format"].as_str().map(str::to_string),
            is_remote: row["is_remote"].as_i64().unwrap_or(0) != 0,
            exists: row["is_missing"].as_i64().unwrap_or(0) == 0,
            languages: Vec::new(),
        })
        .collect();
    let selected = selection::select(&candidates, &PlaybackPrefs::default());
    Json(json!({ "selected_file_id": selected, "files": rows })).into_response()
}

#[derive(Deserialize)]
struct PinRequest {
    file_id: i64,
    #[serde(default)]
    window_start: Option<String>,
}

/// Pin one file for an item's playout window.
async fn pin_file(
    State(state): State<Arc<RouteState>>,
    Path(id): Path<i64>,
    Json(input): Json<PinRequest>,
) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.save_pin(id, input.file_id, input.window_start.as_deref()).await {
        Ok(pin_id) => {
            Json(json!({ "pin": { "id": pin_id, "item_id": id, "file_id": input.file_id } }))
                .into_response()
        }
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

/// The last sync runs, newest first.
async fn health_events(State(state): State<Arc<RouteState>>) -> Response {
    let media = MediaDb::new(state.db.clone());
    match media.recent_sync_runs(20).await {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(error) => fail(StatusCode::INTERNAL_SERVER_ERROR, error.0),
    }
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}