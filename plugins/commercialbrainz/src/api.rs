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
        .route("/pool", get(pool_preview))
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
        Ok(response) => {
            let status = response.status().as_u16();
            if (200..=299).contains(&status) {
                json!({ "ok": true, "code": "ok", "detail": "Reachable, token accepted" })
            } else if status == 401 || status == 403 {
                // Read the server's own message — a rejection can be about the
                // request itself (a 403 named the missing User-Agent before),
                // which tells the user far more than "token rejected".
                let server_detail = response
                    .text()
                    .await
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                    .and_then(|value| value.get("detail").and_then(|detail| detail.as_str()).map(str::to_string));
                json!({
                    "ok": false,
                    "code": "auth_failed",
                    "detail": server_detail
                        .unwrap_or_else(|| "Server reachable, API token rejected".to_string()),
                })
            } else {
                json!({ "ok": false, "code": "bad_url", "detail": format!("Unexpected response ({status})") })
            }
        }
    };
    Json(json!({ "result": result })).into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Pull the pool the settings would sync the way v1.0.0 did: page through the
/// browse endpoint up to the max results and apply the saved filters. The
/// page shows how many would come in, why others were filtered, and a sample.
async fn pool_preview(State(state): State<Arc<CommercialBrainzState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    if !settings.enabled {
        return Json(json!({ "ok": false, "error": "CommercialBrainz is disabled." })).into_response();
    }
    let mut offset: i64 = 0;
    let page_size: i64 = 50;
    let max_results = settings.max_sync_results.max(1) as i64;
    let mut accepted: Vec<serde_json::Value> = Vec::new();
    let mut filtered = 0i64;
    let mut reasons: Vec<(String, i64)> = Vec::new();
    let mut total_on_server = 0i64;
    let mut reached_end = false;

    while accepted.len() < max_results as usize {
        match crate::pool::browse(&state.http, &settings, offset, page_size).await {
            Ok(page) => {
                total_on_server = page.total;
                if page.items.is_empty() {
                    reached_end = true;
                    break;
                }
                for item in page.items {
                    match crate::pool::filter(&settings, &item) {
                        Ok(()) => accepted.push(crate::pool::item_view(&item, &settings.base_url)),
                        Err(reason) => {
                            filtered += 1;
                            if let Some(slot) = reasons.iter_mut().find(|(name, _)| name == reason) {
                                slot.1 += 1;
                            } else {
                                reasons.push((reason.to_string(), 1));
                            }
                        }
                    }
                    if accepted.len() >= max_results as usize {
                        break;
                    }
                }
                offset += page_size;
                if offset >= total_on_server || offset >= 2000 {
                    reached_end = true;
                    break;
                }
            }
            Err(error) => {
                return Json(json!({ "ok": false, "error": error })).into_response();
            }
        }
    }

    let sample = accepted.iter().take(24).cloned().collect::<Vec<_>>();
    Json(json!({
        "ok": true,
        "enabled": settings.enabled,
        "base_url": settings.base_url,
        "total_on_server": total_on_server,
        "accepted": accepted.len(),
        "filtered": filtered,
        "reasons": reasons,
        "paged_fully": reached_end,
        "sample": sample,
    }))
    .into_response()
}