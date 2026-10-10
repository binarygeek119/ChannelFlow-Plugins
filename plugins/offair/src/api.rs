//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.offair`, so the Off Air page's calls to
//! `GET /` and `PUT /` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::settings::{OffAirSettings, SETTINGS_KEY};
use crate::OffAirState;

pub fn router(state: Arc<OffAirState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .with_state(state)
}

/// The settings plus the option lists the form renders from, so the shell
/// never hard-codes the choices.
fn view(settings: &OffAirSettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "display_modes": ["slate_image", "color_bars", "static"],
            "slate_variants": ["usa", "international"],
            "audio_modes": ["background_music", "white_noise", "silence", "beep_tone"],
            "music_sources": ["all_music_libraries", "named_library", "local_packs"],
        }
    })
}

async fn get_settings(State(state): State<Arc<OffAirState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<OffAirState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let settings = match OffAirSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("off-air settings updated");
    Json(view(&settings)).into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}