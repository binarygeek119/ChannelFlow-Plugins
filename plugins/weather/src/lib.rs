//! The Weather plugin.
//!
//! 1.0.0's Weather tab as a plugin: the WeatherStar look, the weather source,
//! the default location, units, and which screens play in the loop. The
//! settings live in the plugin's own namespaced storage and the Weather tab
//! in the nav is the `page` contribution this plugin declares — install the
//! plugin and the tab appears.

mod api;
mod forecast;
mod guide;
mod settings;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::contrib::PresetContribution;
use channelflow_plugin_api::core::CoreData;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::ui::UiContribution;
use channelflow_plugin_api::{register_preset, unregister_preset};
use tokio::sync::Mutex;

use crate::settings::{WeatherSettings, SETTINGS_KEY};

/// The ready-made WeatherStar channels this plugin contributes to the Presets
/// page. They are registered on load, so they appear only while this plugin
/// is installed.
fn weather_presets() -> Vec<PresetContribution> {
    let logo = "/plugin/com.channelflow.weather/web/logos/WeatherStar.png".to_string();
    vec![
        PresetContribution {
            id: "channelflow-weatherstar4000".to_string(),
            plugin_id: "com.channelflow.weather".to_string(),
            legacy: 126.0,
            subchannel: 119.5,
            name: "WeatherStar4000".to_string(),
            category: "Weather".to_string(),
            description: "Live WeatherStar 4000+ MPEG-TS weather channel".to_string(),
            logo: logo.clone(),
        },
        PresetContribution {
            id: "channelflow-weatherstar3000".to_string(),
            plugin_id: "com.channelflow.weather".to_string(),
            legacy: 126.0,
            subchannel: 119.5,
            name: "WeatherStar3000".to_string(),
            category: "Weather".to_string(),
            description: "Live WeatherStar 3000+ MPEG-TS weather channel".to_string(),
            logo,
        },
    ]
}

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(WeatherPlugin::new())
}

pub struct WeatherPlugin {
    metadata: PluginManifest,
    state: Option<Arc<WeatherState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct WeatherState {
    pub settings: Mutex<WeatherSettings>,
    pub storage: Arc<dyn PluginStorage>,
    pub http: reqwest::Client,
    pub logger: PluginLogger,
    /// The last fetched snapshot, keyed by its (query, source, units), with
    /// the instant it was taken — v1.0.0 cached weather for ~8 minutes.
    pub weather_cache: Mutex<Option<(String, std::time::Instant, serde_json::Value)>>,
    /// Read channels (to find the WeatherStar ones) and write their guide.
    pub core: Arc<dyn CoreData>,
}

impl WeatherPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<WeatherState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the Weather plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for WeatherPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // Offer itself to the Presets page: WeatherStar4000 and
        // WeatherStar3000 become presets while this plugin is installed, and
        // the WeatherStar logo is their channel watermark.
        for preset in weather_presets() {
            register_preset(preset);
        }
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("logos/WeatherStar.png", "image/png", include_bytes!("../web/logos/WeatherStar.png"));
        api.web.serve_embedded("weather.css", "text/css", include_bytes!("../web/weather.css"));
        api.web.serve_embedded("weather.html", "text/html", include_bytes!("../web/weather.html"));
        api.web.serve_embedded("weather.js", "text/javascript", include_bytes!("../web/weather.js"));

        let settings = match api
            .storage
            .get(SETTINGS_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(value) => {
                WeatherSettings::parse(&value).map_err(|error| PluginError::new(error.to_string()))?
            }
            None => WeatherSettings::default(),
        };
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| PluginError::new(format!("building the HTTP client: {error}")))?;
        self.state = Some(Arc::new(WeatherState {
            settings: Mutex::new(settings),
            storage: api.storage.clone(),
            http,
            logger: api.logger.clone(),
            weather_cache: Mutex::new(None),
            core: api.core.clone(),
        }));
        // The daily guide task: once a day at the configured time, write the
        // day's forecast into the WeatherStar channel's guide.
        if let Some(state) = self.state.clone() {
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    ticker.tick().await;
                    let settings = state.settings.lock().await.clone();
                    if !settings.guide_enabled {
                        continue;
                    }
                    let last = guide::last_build_date(&state).await;
                    if !guide::due(&settings, chrono::Local::now(), last.as_deref()) {
                        continue;
                    }
                    match guide::build(&state, false).await {
                        Ok(count) => {
                            guide::record_build(&state).await;
                            state
                                .logger
                                .info(&format!("weather guide built {count} blocks"));
                        }
                        Err(error) => state
                            .logger
                            .warn(&format!("weather guide build failed: {error}")),
                    }
                }
            });
        }
        api.logger.info("loaded");
        Ok(())
    }

    async fn on_enable(&mut self) -> Result<(), PluginError> {
        self.enabled = true;
        Ok(())
    }

    async fn on_disable(&mut self) -> Result<(), PluginError> {
        self.enabled = false;
        Ok(())
    }

    fn on_unload(&mut self) {
        // The WeatherStar presets disappear with it: they are in the preset
        // list only while this plugin is installed.
        for preset in weather_presets() {
            unregister_preset(&preset.id);
        }
        self.state = None;
    }

    async fn on_config(&mut self, config: serde_json::Value) -> Result<(), PluginError> {
        let settings =
            WeatherSettings::parse(&config).map_err(|error| PluginError::new(error.to_string()))?;
        let state = self.state()?.clone();
        let stored = serde_json::to_value(&settings)
            .map_err(|error| PluginError::new(error.to_string()))?;
        state
            .storage
            .set(SETTINGS_KEY, &stored)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?;
        *state.settings.lock().await = settings;
        Ok(())
    }

    fn routes(&self) -> Option<Router> {
        self.state.as_ref().map(|state| api::router(state.clone()))
    }

    fn ui_contributions(&self) -> Vec<UiContribution> {
        self.metadata.ui_contributions.clone()
    }

    fn health(&self) -> PluginHealth {
        PluginHealth {
            ok: self.enabled,
            detail: if self.enabled {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
        }
    }
}

impl Default for WeatherPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelflow_plugin_api::storage::InMemoryStorage;

    #[test]
    fn bundled_manifest_is_valid_and_declares_a_page() {
        let plugin = WeatherPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.weather");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata.ui_contributions.iter().any(
            |contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "weather")
        ));
    }

    #[tokio::test]
    async fn loads_defaults_on_first_run() {
        let mut plugin = WeatherPlugin::new();
        let api = PluginApi {
            id: "com.channelflow.weather".to_string(),
            storage: Arc::new(InMemoryStorage::new()),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.weather"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
            database: Arc::new(channelflow_plugin_api::database::NoPluginDatabase),
            web: channelflow_plugin_api::PluginWeb::new(),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.weatherstar_variant, settings::StarVariant::Ws4kp);
    }
}