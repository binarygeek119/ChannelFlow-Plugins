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
use channelflow_plugin_api::plugin::PluginLogger;
use serde::Deserialize;
use serde_json::json;

use crate::presets::PRESETS;

pub struct RoutesState {
    pub core: Arc<dyn CoreData>,
    pub logger: PluginLogger,
}

pub fn router(core: Arc<dyn CoreData>, logger: PluginLogger) -> Router {
    Router::new()
        .route("/presets", get(list_presets))
        .with_state(Arc::new(RoutesState { core, logger }))
}

#[derive(Deserialize)]
struct PresetsQuery {
    /// Which dial numbering the page is using: "retro" (the original whole
    /// numbers, 119) or "modern" (the v1 subchannels, 119.1). Defaults retro.
    #[serde(default)]
    mode: Option<String>,
    /// Which WeatherStar contributes a channel: "channelflow-weatherstar4000"
    /// (default) or "channelflow-weatherstar3000". Only meaningful when both
    /// are contributed (the Weather plugin is installed); then exactly one
    /// WeatherStar preset is offered for creation.
    #[serde(default)]
    weatherstar: Option<String>,
}

const WEATHERSTAR_4000: &str = "channelflow-weatherstar4000";
const WEATHERSTAR_3000: &str = "channelflow-weatherstar3000";

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
#[derive(Clone)]
struct PresetRow<'a> {
    id: &'a str,
    legacy: f64,
    subchannel: f64,
    name: &'a str,
    category: &'a str,
    description: &'a str,
    /// The v1.0.0 artwork, relative to this plugin's web root, or "".
    logo: &'a str,
}

/// The logo a contributed preset uses (the presets plugin hosts the v1 set).
fn contributed_logo(id: &str) -> &'static str {
    match id {
        "channelflow-live-news" => "logos/News/FlowWire.png",
        "channelflow-weatherstar4000" => "logos/Weather/WeatherStar4000.png",
        "channelflow-weatherstar3000" => "logos/Weather/WeatherStar3000.png",
        "channelflow-holiday" => "logos/The_Holiday_Channel/The Holiday Channel-plane.png",
        _ => "",
    }
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
        "logo": row.logo,
        "exists": exists,
        // The Scheduler layers this preset applies once its channel exists;
        // null for plugin-contributed presets, which bring their own behaviour.
        "layers": crate::layers::layers_for(row.id),
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
    state.logger.info("presets: listing all presets");
    
    let mode = query.mode.as_deref().unwrap_or("retro");
    let channels = match state.core.channels().await {
        Ok(channels) => channels,
        Err(error) => {
            state.logger.warn(&format!("presets: failed to fetch channels: {}", error));
            Vec::new()
        }
    };
    state.logger.info(&format!("presets: found {} existing channels", channels.len()));
    
    let mut rows: Vec<PresetRow<'_>> = PRESETS
        .iter()
        .map(|preset| PresetRow {
            id: preset.id,
            legacy: preset.legacy,
            subchannel: preset.subchannel,
            name: preset.name,
            category: preset.category,
            description: preset.description,
            logo: preset.logo,
        })
        .collect();
    
    state.logger.info(&format!("presets: loaded {} built-in presets", rows.len()));
    
    // The ready-made channels other plugins contribute (The Holiday Channel
    // registers itself here on load). They are only listed while their owning
    // plugin is actually installed.
    let installed = match state.core.installed_plugins().await {
        Ok(installed) => installed.into_iter().collect::<std::collections::HashSet<_>>(),
        Err(error) => {
            state.logger.warn(&format!("presets: failed to fetch installed plugins: {}", error));
            std::collections::HashSet::new()
        }
    };
    
    let contributed_values = channelflow_plugin_api::contributed_presets();
    let contributed: Vec<PresetRow<'_>> = contributed_values
        .iter()
        .filter(|preset| installed.contains(&preset.plugin_id))
        .map(|preset| PresetRow {
            id: &preset.id,
            legacy: preset.legacy,
            subchannel: preset.subchannel,
            name: &preset.name,
            category: &preset.category,
            description: &preset.description,
            logo: if preset.logo.is_empty() {
                contributed_logo(&preset.id)
            } else {
                &preset.logo
            },
        })
        .collect();
    
    state.logger.info(&format!("presets: found {} contributed presets from installed plugins", contributed.len()));
    
    // The two WeatherStars are one channel, not two: when both are contributed
    // (the Weather plugin is installed) exactly one becomes a preset row, the
    // one the page's switch has selected.
    let both_weatherstars =
        contributed.iter().any(|row| row.id == WEATHERSTAR_4000)
            && contributed.iter().any(|row| row.id == WEATHERSTAR_3000);
    let selected_weatherstar = if both_weatherstars
        && query.weatherstar.as_deref() == Some(WEATHERSTAR_3000)
    {
        WEATHERSTAR_3000
    } else if both_weatherstars {
        WEATHERSTAR_4000
    } else {
        ""
    };
    
    let weatherstar_options: Vec<serde_json::Value> = if both_weatherstars {
        contributed
            .iter()
            .filter(|row| row.id == WEATHERSTAR_4000 || row.id == WEATHERSTAR_3000)
            .map(|row| {
                json!({
                    "id": row.id,
                    "name": row.name,
                    "retro": fmt_number(row.legacy),
                    "modern": fmt_number(row.subchannel),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    
    for row in contributed {
        let swap = both_weatherstars
            && (row.id == WEATHERSTAR_4000 || row.id == WEATHERSTAR_3000);
        if swap {
            if row.id == selected_weatherstar {
                rows.push(row);
            }
            continue;
        }
        rows.push(row);
    }
    
    let presets: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| preset_row_json(PresetRow {
            id: row.id,
            legacy: row.legacy,
            subchannel: row.subchannel,
            name: row.name,
            category: row.category,
            description: row.description,
            logo: row.logo,
        }, &channels, mode))
        .collect();
    
    let exists_count = presets.iter().filter(|p| p["exists"].as_bool().unwrap_or(false)).count();
    state.logger.info(&format!(
        "presets: returning {} presets ({} already exist), mode={}",
        presets.len(),
        exists_count,
        mode
    ));
    
    Json(json!({
        "mode": mode,
        "presets": presets,
        "weatherstar": if both_weatherstars {
            json!({ "options": weatherstar_options, "selected": selected_weatherstar })
        } else {
            serde_json::Value::Null
        },
        "note": "Presets are a quick start — channels can also be added on the Channels tab.",
    }))
}