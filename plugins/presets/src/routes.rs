//! The Presets plugin's API routes, mounted by the base under
//! `/api/plugins/com.channelflow.presets`.

use std::sync::Arc;

use axum::{
    extract::State,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use channelflow_plugin_api::core::CoreData;
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

/// Every ready-made preset and whether a channel already covers its number or
/// name. The page creates the missing ones through the base's own channel API.
async fn list_presets(State(state): State<Arc<RoutesState>>) -> impl IntoResponse {
    let channels = match state.core.channels().await {
        Ok(channels) => channels,
        Err(_) => Vec::new(),
    };
    let presets: Vec<serde_json::Value> = PRESETS
        .iter()
        .map(|preset| {
            let exists = channels.iter().any(|channel| {
                channel.number == preset.number
                    || channel.name.trim().eq_ignore_ascii_case(preset.name.trim())
            });
            json!({
                "id": preset.id,
                "number": preset.number,
                "name": preset.name,
                "category": preset.category,
                "description": preset.description,
                "exists": exists,
            })
        })
        .collect();
    Json(json!({
        "presets": presets,
        "note": "Presets are a quick start — channels can also be added on the Channels tab.",
    }))
}