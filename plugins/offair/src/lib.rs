//! The Off Air plugin.
//!
//! What plays when a channel has no scheduled media, when playback fails, or
//! when a weather capture errors. This is 1.0.0's EBS feature lifted into a
//! plugin: video and audio are chosen independently (a slate image, color
//! bars, or static; background music, white noise, silence, or a beep). The
//! settings live in the plugin's own namespaced storage and the Off Air tab
//! in the nav is the `page` contribution this plugin declares — install the
//! plugin and the tab appears.

mod api;
mod settings;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::ui::UiContribution;
use channelflow_plugin_api::{manifest::PluginManifest, PLUGIN_ABI_VERSION};
use tokio::sync::Mutex;

use crate::settings::{OffAirSettings, SETTINGS_KEY};

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(OffAirPlugin::new())
}

/// The ABI entrypoint, for a future dynamic loader. Compiled only for the
/// `abi` feature (the standalone cdylib) so compiled-in plugins do not collide
/// on the shared symbol name.
#[cfg(feature = "abi")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn channelflow_plugin_v1() -> *mut dyn Plugin {
    Box::into_raw(Box::new(OffAirPlugin::new()))
}

#[cfg(feature = "abi")]
#[no_mangle]
pub static channelflow_plugin_abi_version: u32 = PLUGIN_ABI_VERSION;

pub struct OffAirPlugin {
    metadata: PluginManifest,
    state: Option<Arc<OffAirState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct OffAirState {
    pub settings: Mutex<OffAirSettings>,
    pub storage: Arc<dyn PluginStorage>,
    pub logger: PluginLogger,
}

impl OffAirPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<OffAirState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the Off Air plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for OffAirPlugin {
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
            Some(value) => OffAirSettings::parse(&value)
                .map_err(|error| PluginError::new(error.to_string()))?,
            None => OffAirSettings::default(),
        };
        self.state = Some(Arc::new(OffAirState {
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
            OffAirSettings::parse(&config).map_err(|error| PluginError::new(error.to_string()))?;
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

impl Default for OffAirPlugin {
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
        let plugin = OffAirPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.offair");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata
            .ui_contributions
            .iter()
            .any(|contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "ebs")));
    }

    #[tokio::test]
    async fn loads_defaults_and_keeps_them_in_storage() {
        let storage = InMemoryStorage::new();
        let mut plugin = OffAirPlugin::new();
        let api = PluginApi {
            id: "com.channelflow.offair".to_string(),
            storage: Arc::new(storage),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.offair"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
            database: Arc::new(channelflow_plugin_api::database::NoPluginDatabase),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.display_mode, settings::DisplayMode::SlateImage);
        assert!(!plugin.enabled);
    }
}