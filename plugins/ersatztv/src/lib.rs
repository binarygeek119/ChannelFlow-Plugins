//! The ErsatzTV Transcoding Engine plugin.
//!
//! Everything that used to be the Transcode page in the base moved here: the
//! instance defaults and each channel's overrides over them, all mirrored
//! exactly from next's `ffmpeg` + `normalization` schema. The Transcode screen
//! stays part of the base shell but talks only to routes this plugin mounts
//! under `/api/plugins/com.channelflow.ersatztv`.
//!
//! The instance defaults live in the plugin's own storage under `defaults`;
//! overrides live under `overrides`, a map of channel id to patch, so nothing
//! core-owned has to change when a channel's encoding is configured. Channels
//! themselves are read through the core-data handle (`api:core:read`).

mod api;
mod transcode;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::{
    core::CoreData, manifest::PluginManifest, plugin::PluginError, Plugin, PluginApi,
    PluginHealth, PluginLogger, PluginStorage, UiContribution,
};
use tokio::sync::Mutex;

use crate::transcode::TranscodeConfig;

/// The key the instance defaults are stored under.
pub const DEFAULTS_KEY: &str = "defaults";
/// The key the per-channel override map is stored under.
pub const OVERRIDES_KEY: &str = "overrides";

fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(ErsatzTvPlugin::new())
}

pub struct ErsatzTvPlugin {
    metadata: PluginManifest,
    state: Option<Arc<ErsatzTvState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct ErsatzTvState {
    pub defaults: Mutex<TranscodeConfig>,
    pub overrides: Mutex<serde_json::Map<String, serde_json::Value>>,
    pub storage: Arc<dyn PluginStorage>,
    pub core: Arc<dyn CoreData>,
    pub logger: PluginLogger,
}

impl ErsatzTvPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<Arc<ErsatzTvState>, PluginError> {
        self.state
            .clone()
            .ok_or_else(|| PluginError::new("ErsatzTV plugin has not been loaded"))
    }
}

impl ErsatzTvState {
    async fn save_defaults(&self, config: &TranscodeConfig) -> Result<(), PluginError> {
        let value = serde_json::to_value(config)
            .map_err(|error| PluginError::new(error.to_string()))?;
        self.storage
            .set(DEFAULTS_KEY, &value)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?;
        *self.defaults.lock().await = config.clone();
        Ok(())
    }

    async fn overrides(&self) -> serde_json::Map<String, serde_json::Value> {
        self.overrides.lock().await.clone()
    }

    async fn save_overrides(
        &self,
        map: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), PluginError> {
        let value = serde_json::Value::Object(map.clone());
        self.storage
            .set(OVERRIDES_KEY, &value)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?;
        *self.overrides.lock().await = map;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Plugin for ErsatzTvPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        let defaults = match api
            .storage
            .get(DEFAULTS_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(value) => TranscodeConfig::parse(&value)
                .map_err(|error| PluginError::new(error.to_string()))?,
            None => TranscodeConfig::default(),
        };
        let overrides = match api
            .storage
            .get(OVERRIDES_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        self.state = Some(Arc::new(ErsatzTvState {
            defaults: Mutex::new(defaults),
            overrides: Mutex::new(overrides),
            storage: api.storage,
            core: api.core,
            logger: api.logger.clone(),
        }));
        api.logger.info("loaded");
        Ok(())
    }

    async fn on_enable(&mut self) -> Result<(), PluginError> {
        if self.enabled {
            return Ok(());
        }
        self.enabled = true;
        if let Some(state) = &self.state {
            let overrides = state.overrides().await;
            state.logger.info(&format!(
                "enabled with {} channel override(s)",
                overrides.len()
            ));
        }
        Ok(())
    }

    async fn on_disable(&mut self) -> Result<(), PluginError> {
        if !self.enabled {
            return Ok(());
        }
        self.enabled = false;
        if let Some(state) = &self.state {
            state.logger.info("disabled");
        }
        Ok(())
    }

    fn on_unload(&mut self) {
        self.state = None;
    }

    async fn on_config(&mut self, config: serde_json::Value) -> Result<(), PluginError> {
        let state = self.state()?;
        let parsed = TranscodeConfig::parse(&config)
            .map_err(|error| PluginError::new(error.to_string()))?;
        state.save_defaults(&parsed).await?;
        Ok(())
    }

    fn routes(&self) -> Option<Router> {
        self.state().ok().map(api::router)
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

#[cfg(test)]
mod tests {
    use super::*;
    use channelflow_plugin_api::storage::InMemoryStorage;

    #[test]
    fn bundled_manifest_is_valid_and_compatible() {
        let plugin = ErsatzTvPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.ersatztv");
        assert!(metadata.compatible_with("2.0.0"));
    }

    #[tokio::test]
    async fn loads_defaults_from_storage() {
        let mut storage = InMemoryStorage::new();
        let defaults = TranscodeConfig::default();
        storage.seed(
            DEFAULTS_KEY,
            serde_json::to_value(&defaults).expect("serialize"),
        );

        let mut plugin = ErsatzTvPlugin::new();
        let api = PluginApi {
            id: metadata_id(),
            storage: Arc::new(storage),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.ersatztv"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
        };
        plugin.on_load(api).await.expect("load");
        let state = plugin.state().expect("state");
        assert_eq!(*state.defaults.lock().await, defaults);
        assert!(state.overrides().await.is_empty());

        plugin.on_enable().await.expect("enable");
        assert!(plugin.health().ok);
    }

    fn metadata_id() -> String {
        "com.channelflow.ersatztv".to_string()
    }
}