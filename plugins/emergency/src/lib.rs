//! The Emergency Broadcast System plugin.
//!
//! 1.0.0's weather-alert overlay as a plugin: whether alerts display (off,
//! cut-in, or a scrolling ticker), and when they cut in, how often and for
//! how long. The settings live in the plugin's own namespaced storage and the
//! Emergency Broadcast System tab in the nav is the `page` contribution this
//! plugin declares — install the plugin and the tab appears.

mod api;
mod settings;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::ui::UiContribution;
use tokio::sync::Mutex;

use crate::settings::{EmergencySettings, SETTINGS_KEY};

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(EmergencyPlugin::new())
}

pub struct EmergencyPlugin {
    metadata: PluginManifest,
    state: Option<Arc<EmergencyState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct EmergencyState {
    pub settings: Mutex<EmergencySettings>,
    pub storage: Arc<dyn PluginStorage>,
    pub logger: PluginLogger,
}

impl EmergencyPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<EmergencyState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the Emergency plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for EmergencyPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("emergency.css", "text/css", include_bytes!("../web/emergency.css"));
        api.web.serve_embedded("emergency.html", "text/html", include_bytes!("../web/emergency.html"));
        api.web.serve_embedded("emergency.js", "text/javascript", include_bytes!("../web/emergency.js"));

        let settings = match api
            .storage
            .get(SETTINGS_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(value) => EmergencySettings::parse(&value)
                .map_err(|error| PluginError::new(error.to_string()))?,
            None => EmergencySettings::default(),
        };
        self.state = Some(Arc::new(EmergencyState {
            settings: Mutex::new(settings),
            storage: api.storage.clone(),
            logger: api.logger.clone(),
        }));
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
        self.state = None;
    }

    async fn on_config(&mut self, config: serde_json::Value) -> Result<(), PluginError> {
        let settings =
            EmergencySettings::parse(&config).map_err(|error| PluginError::new(error.to_string()))?;
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

impl Default for EmergencyPlugin {
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
        let plugin = EmergencyPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.emergency");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata.ui_contributions.iter().any(
            |contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "emergency")
        ));
    }

    #[tokio::test]
    async fn loads_defaults_on_first_run() {
        let mut plugin = EmergencyPlugin::new();
        let api = PluginApi {
            id: "com.channelflow.emergency".to_string(),
            storage: Arc::new(InMemoryStorage::new()),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.emergency"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
            database: Arc::new(channelflow_plugin_api::database::NoPluginDatabase),
            web: channelflow_plugin_api::PluginWeb::new(),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.alert_display, settings::AlertDisplay::Off);
    }
}