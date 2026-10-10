//! The News plugin.
//!
//! 1.0.0's FlowWire newscast as a plugin: the header, article count, refresh
//! interval, TTS voice and engine, anchor intro/outro, the bulletin schedule,
//! the RSS feeds, and a headline preview. The settings live in the plugin's
//! own namespaced storage and the News tab in the nav is the `page`
//! contribution this plugin declares — install the plugin and the tab
//! appears.

mod api;
mod settings;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::ui::UiContribution;
use tokio::sync::Mutex;

use crate::settings::{NewsSettings, SETTINGS_KEY};

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(NewsPlugin::new())
}

pub struct NewsPlugin {
    metadata: PluginManifest,
    state: Option<Arc<NewsState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct NewsState {
    pub settings: Mutex<NewsSettings>,
    pub storage: Arc<dyn PluginStorage>,
    pub http: reqwest::Client,
    pub logger: PluginLogger,
}

impl NewsPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<NewsState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the News plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for NewsPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        let settings = match api
            .storage
            .get(SETTINGS_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(value) => {
                NewsSettings::parse(&value).map_err(|error| PluginError::new(error.to_string()))?
            }
            None => NewsSettings::default(),
        };
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|error| PluginError::new(format!("building the HTTP client: {error}")))?;
        self.state = Some(Arc::new(NewsState {
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
        let settings =
            NewsSettings::parse(&config).map_err(|error| PluginError::new(error.to_string()))?;
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

impl Default for NewsPlugin {
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
        let plugin = NewsPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.news");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata.ui_contributions.iter().any(
            |contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "news")
        ));
    }

    #[tokio::test]
    async fn loads_defaults_on_first_run() {
        let mut plugin = NewsPlugin::new();
        let api = PluginApi {
            id: "com.channelflow.news".to_string(),
            storage: Arc::new(InMemoryStorage::new()),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.news"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
            database: Arc::new(channelflow_plugin_api::database::NoPluginDatabase),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.header, "FlowWire News");
    }
}