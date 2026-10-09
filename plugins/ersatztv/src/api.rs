//! The API this plugin mounts. The base nests this router under
//! `/api/plugins/com.channelflow.ersatztv`, so the Transcode page's calls to
//! `GET /` and `GET /channels/{id}` resolve there.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::transcode::{self, TranscodeConfig};
use crate::ErsatzTvState;

pub fn router(state: Arc<ErsatzTvState>) -> Router {
    Router::new()
        .route("/", get(get_defaults).put(put_defaults))
        .route("/channels", get(list_channels))
        .route(
            "/channels/{id}",
            get(get_channel).put(put_channel).delete(clear_channel),
        )
        .route("/channels/{id}/changes", get(channel_changes))
        .with_state(state)
}

type StateRef = Arc<ErsatzTvState>;

/// Plugin-handled request failures, translated into HTTP status codes.
pub struct HttpError {
    status: StatusCode,
    message: String,
}

impl HttpError {
    fn bad(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn missing_channel(id: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: format!("no channel with id {id}"),
        }
    }
}

impl From<crate::transcode::TranscodeError> for HttpError {
    fn from(error: crate::transcode::TranscodeError) -> Self {
        HttpError::bad(error.to_string())
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

fn storage_error(error: channelflow_plugin_api::plugin::PluginError) -> HttpError {
    HttpError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("could not save the transcode settings: {error}"),
    }
}

/// The instance defaults plus the field list the Transcode page renders.
async fn get_defaults(State(state): State<StateRef>) -> Result<Json<serde_json::Value>, HttpError> {
    let defaults = state.defaults.lock().await.clone();
    Ok(Json(json!({
        "spec": transcode::spec(),
        "defaults": defaults,
    })))
}

async fn put_defaults(
    State(state): State<StateRef>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let config = TranscodeConfig::parse(&body)?;
    state
        .save_defaults(&config)
        .await
        .map_err(storage_error)?;
    state
        .audit("defaults_changed", None, Some(&serde_json::to_value(&config).unwrap_or_default()))
        .await;
    Ok(Json(json!({ "defaults": config })))
}

/// Every channel, as the core sees it, so the Transcode page can list which
/// channels have overrides.
async fn list_channels(
    State(state): State<StateRef>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let channels = state
        .core
        .channels()
        .await
        .map_err(|error| HttpError::bad(error.to_string()))?;
    let overrides = state.overrides().await;
    Ok(Json(json!({
        "channels": channels.iter().map(|channel| {
            json!({
                "id": channel.id,
                "number": channel.number,
                "name": channel.name,
                "enabled": channel.enabled,
                "overridden": overrides.contains_key(&channel.id),
            })
        }).collect::<Vec<_>>(),
    })))
}

/// Find one channel so the dialog can show its name even though the settings
/// live with the plugin.
async fn find_channel(
    state: &StateRef,
    id: &str,
) -> Result<channelflow_plugin_api::core::CoreChannel, HttpError> {
    let channels = state
        .core
        .channels()
        .await
        .map_err(|error| HttpError::bad(error.to_string()))?;
    channels
        .into_iter()
        .find(|channel| channel.id == id)
        .ok_or_else(|| HttpError::missing_channel(id))
}

async fn get_channel(
    State(state): State<StateRef>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let channel = find_channel(&state, &id).await?;
    let defaults = state.defaults.lock().await.clone();
    let overrides = channel_overrides(&state, &id).await;
    let effective = defaults.merged(&overrides)?;
    Ok(Json(json!({
        "channel": { "id": channel.id, "number": channel.number, "name": channel.name },
        "spec": transcode::spec(),
        "defaults": defaults,
        "overrides": overrides,
        "effective": effective,
    })))
}

async fn put_channel(
    State(state): State<StateRef>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, HttpError> {
    find_channel(&state, &id).await?;
    let defaults = state.defaults.lock().await.clone();
    // Resolve before storing, so a patch next would reject never reaches the
    // override map.
    let effective = defaults.merged(&body)?;

    let mut map = state.overrides().await;
    let patch = if body.is_null() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        body
    };
    map.insert(id.clone(), patch.clone());
    state.save_overrides(map).await.map_err(storage_error)?;
    state.audit("overrides_changed", Some(&id), Some(&patch)).await;
    Ok(Json(json!({
        "overrides": channel_overrides(&state, &id).await,
        "effective": effective,
    })))
}

/// Drop a channel's overrides so it follows the Transcode page again.
async fn clear_channel(
    State(state): State<StateRef>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    find_channel(&state, &id).await?;
    let mut map = state.overrides().await;
    map.remove(&id);
    state.save_overrides(map).await.map_err(storage_error)?;
    state.audit("overrides_cleared", Some(&id), None).await;
    let defaults = state.defaults.lock().await.clone();
    Ok(Json(json!({
        "overrides": serde_json::Value::Object(serde_json::Map::new()),
        "effective": defaults,
    })))
}

/// The most recent changes for one channel, read back from the plugin's own
/// audit table.
async fn channel_changes(
    State(state): State<StateRef>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    find_channel(&state, &id).await?;
    let Some(table) = &state.audit else {
        return Ok(Json(json!({ "available": false, "changes": [] })));
    };
    let sql = format!(
        "SELECT action, at, payload FROM {table} \
         WHERE channel_id = '{}' ORDER BY id DESC LIMIT 25",
        crate::quote(&id)
    );
    let changes = state
        .database
        .fetch(&sql)
        .await
        .map_err(|error| HttpError::bad(error.to_string()))?;
    Ok(Json(json!({ "available": true, "changes": changes })))
}

/// A channel's patch, or `{}` when the channel follows the defaults.
async fn channel_overrides(state: &StateRef, id: &str) -> serde_json::Value {
    state
        .overrides()
        .await
        .get(id)
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()))
}