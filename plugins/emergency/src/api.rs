//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.emergency`, so the Emergency Broadcast
//! System page's calls to `GET /`, `PUT /`, and `POST /test` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::settings::{AlertDisplay, EmergencySettings, SETTINGS_KEY};
use crate::EmergencyState;

pub fn router(state: Arc<EmergencyState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/test", get(test_alerts))
        .with_state(state)
}

fn view(settings: &EmergencySettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "alert_displays": ["off", "cutin", "ticker"],
        }
    })
}

async fn get_settings(State(state): State<Arc<EmergencyState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<EmergencyState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let settings = match EmergencySettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("emergency settings updated");
    Json(view(&settings)).into_response()
}

/// Simulate an alert so the settings page can show what an active alert would
/// do, without needing a live NOAA watch.
async fn test_alerts(State(state): State<Arc<EmergencyState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let (kind, detail): (&str, String) = match settings.alert_display {
        AlertDisplay::Off => (
            "off",
            "Alerts are off — no overlay will appear during programming.".to_string(),
        ),
        AlertDisplay::CutIn => (
            "cutin",
            format!(
                "Alerts screen every {} minutes for {} seconds; show audio ducks under the attention/end tones.",
                settings.cutin_interval_minutes,
                settings.cutin_duration_seconds
            ),
        ),
        AlertDisplay::Ticker => (
            "ticker",
            "Scrolling alert text overlays the bottom of the current program.".to_string(),
        ),
    };
    Json(json!({
        "result": {
            "ok": settings.alert_display != AlertDisplay::Off,
            "display": kind,
            "detail": detail,
        }
    }))
    .into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}