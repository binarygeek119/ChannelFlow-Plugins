//! The CommercialBrainz plugin.
//!
//! 1.0.0's ad-avail sync connection as a plugin: the server and API token,
//! the pool mode, how many results to sync, and the content filters. The
//! settings live in the plugin's own namespaced storage, the CommercialBrainz
//! tab in the nav is the `page` contribution this plugin declares — install
//! the plugin and the tab appears.

mod api;
mod settings;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::ui::UiContribution;
use channelflow_plugin_api::manifest::PluginManifest;
use tokio::sync::Mutex;

use crate::settings::{CommercialBrainzSettings, SETTINGS_KEY};

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(CommercialBrainzPlugin::new())
}

pub struct CommercialBrainzPlugin {
    metadata: PluginManifest,
    state: Option<Arc<CommercialBrainzState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct CommercialBrainzState {
    pub settings: Mutex<CommercialBrainzSettings>,
    pub storage: Arc<dyn PluginStorage>,
    pub http: reqwest::Client,
    pub logger: PluginLogger,
}

impl CommercialBrainzPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<CommercialBrainzState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the CommercialBrainz plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for CommercialBrainzPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("commercialbrainz.css", "text/css", include_bytes!("../web/commercialbrainz.css"));
        api.web.serve_embedded("commercialbrainz.html", "text/html", include_bytes!("../web/commercialbrainz.html"));
        api.web.serve_embedded("commercialbrainz.js", "text/javascript", include_bytes!("../web/commercialbrainz.js"));

        let settings = match api
            .storage
            .get(SETTINGS_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(value) => CommercialBrainzSettings::parse(&value)
                .map_err(|error| PluginError::new(error.to_string()))?,
            None => CommercialBrainzSettings::default(),
        };
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| PluginError::new(format!("building the HTTP client: {error}")))?;
        self.state = Some(Arc::new(CommercialBrainzState {
            settings: Mutex::new(settings),
            storage: api.storage.clone(),
            http,
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
        let settings = CommercialBrainzSettings::parse(&config)
            .map_err(|error| PluginError::new(error.to_string()))?;
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

impl Default for CommercialBrainzPlugin {
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
        let plugin = CommercialBrainzPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.commercialbrainz");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata.ui_contributions.iter().any(
            |contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "commercialbrainz")
        ));
    }

    #[tokio::test]
    async fn loads_defaults_on_first_run() {
        let mut plugin = CommercialBrainzPlugin::new();
        let api = PluginApi {
            id: "com.channelflow.commercialbrainz".to_string(),
            storage: Arc::new(InMemoryStorage::new()),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.commercialbrainz"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
            database: Arc::new(channelflow_plugin_api::database::NoPluginDatabase),
            web: channelflow_plugin_api::PluginWeb::new(),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.base_url, settings::DEFAULT_BASE_URL);
    }
}