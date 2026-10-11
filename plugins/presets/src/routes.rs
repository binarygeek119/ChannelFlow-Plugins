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

fn preset_number(legacy: f64, subchannel: f64, mode: &str) -> f64 {
    if mode == "modern" {
        subchannel
    } else {
        legacy
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

/// One preset row for the page: built-in or contributed by another plugin
/// (The Holiday Channel only appears while its plugin is installed).
struct PresetRow<'a> {
    id: &'a str,
    legacy: f64,
    subchannel: f64,
    name: &'a str,
    category: &'a str,
    description: &'a str,
}

fn preset_row_json(
    row: PresetRow<'_>,
    channels: &[channelflow_plugin_api::CoreChannel],
    mode: &str,
) -> serde_json::Value {
    let number = preset_number(row.legacy, row.subchannel, mode);
    let exists = channels.iter().any(|channel| {
        channel.number == number || channel.name.trim().eq_ignore_ascii_case(row.name.trim())
    });
    json!({
        "id": row.id,
        "number": fmt_number(number),
        "retro": fmt_number(row.legacy),
        "modern": fmt_number(row.subchannel),
        "name": row.name,
        "category": row.category,
        "description": row.description,
        "exists": exists,
    })
}

/// Every ready-made preset (built-in and plugin-contributed), its number under
/// both styles, and whether a channel already covers the selected number or
/// its name. The page creates the missing ones through the base's own channel
/// API.
async fn list_presets(
    State(state): State<Arc<RoutesState>>,
    Query(query): Query<PresetsQuery>,
) -> impl IntoResponse {
    let mode = query.mode.as_deref().unwrap_or("retro");
    let channels = match state.core.channels().await {
        Ok(channels) => channels,
        Err(_) => Vec::new(),
    };
    let mut rows: Vec<PresetRow<'_>> = PRESETS
        .iter()
        .map(|preset| PresetRow {
            id: preset.id,
            legacy: preset.legacy,
            subchannel: preset.subchannel,
            name: preset.name,
            category: preset.category,
            description: preset.description,
        })
        .collect();
    // The ready-made channels other plugins contribute (The Holiday Channel
    // registers itself here on load) — they are only present while that plugin
    // is installed.
    let contributed = channelflow_plugin_api::contributed_presets();
    rows.extend(contributed.iter().map(|preset| PresetRow {
        id: &preset.id,
        legacy: preset.legacy,
        subchannel: preset.subchannel,
        name: &preset.name,
        category: &preset.category,
        description: &preset.description,
    }));
    let presets: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| preset_row_json(PresetRow {
            id: row.id,
            legacy: row.legacy,
            subchannel: row.subchannel,
            name: row.name,
            category: row.category,
            description: row.description,
        }, &channels, mode))
        .collect();
    Json(json!({
        "mode": mode,
        "presets": presets,
        "note": "Presets are a quick start — channels can also be added on the Channels tab.",
    }))
}