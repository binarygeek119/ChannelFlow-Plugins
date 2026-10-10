//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.commercialbrainz`, so the CommercialBrainz
//! page's calls to `GET /`, `PUT /`, and `POST /test` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::settings::{CommercialBrainzSettings, SETTINGS_KEY, DEFAULT_BASE_URL};
use crate::CommercialBrainzState;

pub fn router(state: Arc<CommercialBrainzState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/test", get(test_settings))
        .with_state(state)
}

/// The settings plus the option lists the form renders from.
fn view(settings: &CommercialBrainzSettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "pool_modes": ["jellyfin_only", "commercial_brainz_only", "both"],
            "default_base_url": DEFAULT_BASE_URL,
        }
    })
}

async fn get_settings(State(state): State<Arc<CommercialBrainzState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<CommercialBrainzState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let settings = match CommercialBrainzSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("commercialbrainz settings updated");
    Json(view(&settings)).into_response()
}

/// Ping the configured server and report reachability and token acceptance,
/// so the settings page can explain what is wrong.
async fn test_settings(State(state): State<Arc<CommercialBrainzState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let url = format!("{}/api/v1/browse/videos?limit=1", settings.base_url);
    let request = state
        .http
        .get(&url)
        .header("Accept", "application/json");
    let request = if settings.api_token.trim().is_empty() {
        request
    } else {
        request.header("Authorization", format!("Bearer {}", settings.api_token.trim()))
    };
    let result = match request.send().await {
        Err(_) => json!({ "ok": false, "code": "unreachable", "detail": "Could not reach the server" }),
        Ok(response) => match response.status().as_u16() {
            200..=299 => json!({ "ok": true, "code": "ok", "detail": "Reachable, token accepted" }),
            401 | 403 => json!({ "ok": false, "code": "auth_failed", "detail": "Server reachable, API token rejected" }),
            status => json!({ "ok": false, "code": "bad_url", "detail": format!("Unexpected response ({status})") }),
        },
    };
    Json(json!({ "result": result })).into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}