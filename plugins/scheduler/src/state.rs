//! The plugin's own storage: the per-channel layers, the generated days, the
//! day index, and the item history. Shared by the routes and the horizon.

use channelflow_plugin_api::storage::PluginStorage;
use serde_json::{json, Value};

use crate::build::GeneratedDay;
use crate::layers::ResolvedLayers;
use crate::settings::layer_key;

pub fn day_key(channel: &str, date: &str) -> String {
    format!("day_{channel}_{date}")
}

pub fn days_index_key(channel: &str) -> String {
    format!("days_{channel}")
}

pub fn history_key(channel: &str) -> String {
    format!("history_{channel}")
}

pub async fn read_layer(storage: &dyn PluginStorage, channel: &str, layer: &str) -> Value {
    storage
        .get(&layer_key(channel, layer))
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null)
}

pub async fn write_layer(
    storage: &dyn PluginStorage,
    channel: &str,
    layer: &str,
    value: &Value,
) -> Result<(), String> {
    storage
        .set(&layer_key(channel, layer), value)
        .await
        .map_err(|error| error.to_string())
}

/// Read a channel's four layer documents into the resolved shape (before any
/// layer-6 scopes are applied).
pub async fn read_channel_layers(storage: &dyn PluginStorage, channel: &str) -> ResolvedLayers {
    ResolvedLayers {
        hours: serde_json::from_value(read_layer(storage, channel, "hours").await)
            .unwrap_or_default(),
        rules: serde_json::from_value(read_layer(storage, channel, "rules").await)
            .unwrap_or_default(),
        items: serde_json::from_value(read_layer(storage, channel, "items").await)
            .unwrap_or_default(),
        prompt: read_layer(storage, channel, "prompt")
            .await
            .as_str()
            .unwrap_or("")
            .to_string(),
        applied: Vec::new(),
    }
}

pub async fn read_days_index(storage: &dyn PluginStorage, channel: &str) -> Vec<String> {
    storage
        .get(&days_index_key(channel))
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub async fn read_day(
    storage: &dyn PluginStorage,
    channel: &str,
    date: &str,
) -> Option<GeneratedDay> {
    storage
        .get(&day_key(channel, date))
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// Store a day, add it to the channel's index, and append its items to the
/// channel's history (capped) so the next day can avoid repeats.
pub async fn store_day(
    storage: &dyn PluginStorage,
    channel: &str,
    day: &GeneratedDay,
) -> Result<(), String> {
    let value = serde_json::to_value(day).map_err(|error| error.to_string())?;
    storage
        .set(&day_key(channel, &day.date), &value)
        .await
        .map_err(|error| error.to_string())?;

    let mut index = read_days_index(storage, channel).await;
    if !index.contains(&day.date) {
        index.push(day.date.clone());
        index.sort();
        let value = serde_json::to_value(&index).map_err(|error| error.to_string())?;
        storage
            .set(&days_index_key(channel), &value)
            .await
            .map_err(|error| error.to_string())?;
    }

    let mut history: Vec<Value> = storage
        .get(&history_key(channel))
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    for block in &day.blocks {
        for item in &block.items {
            history.push(json!({
                "match_key": item.match_key,
                "date": day.date,
                "start_ms": block.start_ms,
            }));
        }
    }
    const HISTORY_CAP: usize = 2000;
    if history.len() > HISTORY_CAP {
        let drop = history.len() - HISTORY_CAP;
        history.drain(0..drop);
    }
    let value = serde_json::to_value(&history).map_err(|error| error.to_string())?;
    storage
        .set(&history_key(channel), &value)
        .await
        .map_err(|error| error.to_string())
}

pub async fn delete_day(
    storage: &dyn PluginStorage,
    channel: &str,
    date: &str,
) -> Result<(), String> {
    storage
        .delete(&day_key(channel, date))
        .await
        .map_err(|error| error.to_string())?;
    let mut index = read_days_index(storage, channel).await;
    index.retain(|entry| entry != date);
    if let Ok(value) = serde_json::to_value(&index) {
        let _ = storage.set(&days_index_key(channel), &value).await;
    }
    Ok(())
}
