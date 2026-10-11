//! The Presets plugin's API routes, mounted by the base under
//! `/api/plugins/com.channelflow.presets`.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use channelflow_plugin_api::core::CoreData;
use serde::Deserialize;
use serde_json::json;

use crate::presets::PRESETS;

pub struct RoutesState {
    pub core: Arc<dyn CoreData>,
}

pub fn router(core: Arc<dyn CoreData>) -> Router {
    Router::new()
        .route("/presets", get(list_presets))
        .with_state(Arc::new(RoutesState { core }))
}

#[derive(Deserialize)]
struct PresetsQuery {
    /// Which dial numbering the page is using: "retro" (the original whole
    /// numbers, 119) or "modern" (the v1 subchannels, 119.1). Defaults retro.
    #[serde(default)]
    mode: Option<String>,
}

fn preset_number(preset: &crate::presets::Preset, mode: &str) -> f64 {
    if mode == "modern" {
        preset.subchannel
    } else {
        preset.legacy
    }
}

/// A dial number for display: whole numbers stay whole (`119`), subchannels
/// keep their fraction (`119.1`).
fn fmt_number(number: f64) -> String {
    if number.fract() == 0.0 && number.is_finite() {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

/// Every ready-made preset, its number under both styles, and whether a
/// channel already covers the selected number or its name. The page creates
/// the missing ones through the base's own channel API.
async fn list_presets(
    State(state): State<Arc<RoutesState>>,
    Query(query): Query<PresetsQuery>,
) -> impl IntoResponse {
    let mode = query.mode.as_deref().unwrap_or("retro");
    let channels = match state.core.channels().await {
        Ok(channels) => channels,
        Err(_) => Vec::new(),
    };
    let presets: Vec<serde_json::Value> = PRESETS
        .iter()
        .map(|preset| {
            let number = preset_number(preset, mode);
            let exists = channels.iter().any(|channel| {
                channel.number == number
                    || channel.name.trim().eq_ignore_ascii_case(preset.name.trim())
            });
            json!({
                "id": preset.id,
                "number": fmt_number(number),
                "retro": fmt_number(preset.legacy),
                "modern": fmt_number(preset.subchannel),
                "name": preset.name,
                "category": preset.category,
                "description": preset.description,
                "exists": exists,
            })
        })
        .collect();
    Json(json!({
        "mode": mode,
        "presets": presets,
        "note": "Presets are a quick start — channels can also be added on the Channels tab.",
    }))
}