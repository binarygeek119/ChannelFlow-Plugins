//! The Scheduler plugin's settings and the storage keys its layers live under.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::layers::{ItemsLayer, HoursLayer, PromptLayer, Rule, RuleScope};

/// Global settings.
pub const SETTINGS_KEY: &str = "settings";
/// The rule scopes (layer 6), one JSON array.
pub const SCOPES_KEY: &str = "scopes";

/// The scheduler's global settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SchedulerSettings {
    /// How many days ahead to build (1–14).
    pub days_to_build: u32,
    /// Local `HH:MM` the nightly horizon build runs.
    pub build_time: String,
    /// Repair attempts per day before the deterministic fallback.
    pub max_retries: u32,
    /// The most candidates a prompt may carry.
    pub pool_cap: u32,
    /// Build a horizon automatically on the schedule.
    pub enabled: bool,
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            days_to_build: 3,
            build_time: "02:00".to_string(),
            max_retries: 2,
            pool_cap: 300,
            enabled: false,
        }
    }
}

impl SchedulerSettings {
    /// Parse and clamp a saved document.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut settings: SchedulerSettings =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        settings.days_to_build = settings.days_to_build.clamp(1, 14);
        settings.max_retries = settings.max_retries.min(5);
        settings.pool_cap = settings.pool_cap.clamp(1, 2000);
        settings.build_time = settings.build_time.trim().to_string();
        Ok(settings)
    }
}

/// The storage key a channel's layer lives under.
pub fn layer_key(channel_id: &str, layer: &str) -> String {
    format!("channel_{channel_id}_{layer}")
}

/// The four per-channel layer documents, as one bundle for the API. (The
/// per-layer routes read and write each key directly; this is the typed shape.)
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelLayers {
    pub hours: HoursLayer,
    pub rules: Vec<Rule>,
    pub items: ItemsLayer,
    pub prompt: PromptLayer,
}

/// Read the layer-6 scopes.
pub async fn read_scopes(
    storage: &dyn channelflow_plugin_api::storage::PluginStorage,
) -> Vec<RuleScope> {
    storage
        .get(SCOPES_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub async fn write_scopes(
    storage: &dyn channelflow_plugin_api::storage::PluginStorage,
    scopes: &[RuleScope],
) -> Result<(), String> {
    let value = serde_json::to_value(scopes).map_err(|error| error.to_string())?;
    storage
        .set(SCOPES_KEY, &value)
        .await
        .map_err(|error| error.to_string())
}
