//! The plugin's API routes, mounted by the base under
//! `/api/plugins/com.channelflow.jellyfin`. The connection object travels in
//! the request body — the base stores the connections, the plugin never
//! persists keys of its own.
//!
//! The catalog lives in the base now, so there are no item/people/music routes
//! here: the base serves those from its own tables. What is left is what only
//! the plugin can answer — which libraries a server has, and how far a sync
//! has got.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use channelflow_plugin_api::media::Connection;
use serde::Deserialize;
use serde_json::json;

use crate::client::JellyfinClient;

pub fn router() -> Router {
    Router::new()
        .route("/libraries", post(libraries_request))
        .route("/progress", get(progress_snapshot))
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

/// The live sync progress snapshot the popup polls while a sync runs.
async fn progress_snapshot() -> Json<serde_json::Value> {
    let state = crate::sync::progress_state().lock().unwrap();
    Json(serde_json::json!({ "progress": state.clone() }))
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}
