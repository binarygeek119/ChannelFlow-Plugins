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
mod music_pack;
mod settings;

use std::path::PathBuf;
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
    /// The plugin's own config folder (`<config>/plugins/{id}`): custom
    /// off-air slate uploads live under `slates/<variant>.png`.
    pub dir: std::path::PathBuf,
    /// The base config root (`<config>`), where music packs live under
    /// `<config>/music/<id>`.
    pub config_root: PathBuf,
    /// Outbound HTTP, for downloading music packs from Google Drive.
    pub http: reqwest::Client,
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
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("ebs.css", "text/css", include_bytes!("../web/ebs.css"));
        api.web.serve_embedded("ebs.html", "text/html", include_bytes!("../web/ebs.html"));
        api.web.serve_embedded("ebs.js", "text/javascript", include_bytes!("../web/ebs.js"));
        // The offline slates (moved here from the EBS plugin's media).
        serve_offline_media(&api);

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
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|error| PluginError::new(format!("building the HTTP client: {error}")))?;
        // `api.dir` is `<config>/plugins/{id}`, so its grandparent is `<config>`.
        let config_root = api
            .dir
            .parent()
            .and_then(|plugins| plugins.parent())
            .map(PathBuf::from)
            .unwrap_or_else(|| api.dir.clone());
        self.state = Some(Arc::new(OffAirState {
            settings: Mutex::new(settings),
            storage: api.storage.clone(),
            logger: api.logger.clone(),
            dir: api.dir.clone(),
            config_root,
            http,
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

/// Publish the offline slates, under
/// `/plugin/com.channelflow.offair/web/media/<file>`.
fn serve_offline_media(api: &channelflow_plugin_api::plugin::PluginApi) {
    macro_rules! slate {
        ($file:literal) => {
            api.web.serve_embedded(
                concat!("media/", $file),
                "image/jpeg",
                include_bytes!(concat!("../web/media/", $file)),
            )
        };
    }
    slate!("offline_usa_16_9.jpg");
    slate!("offline_usa_4_3.jpg");
    slate!("offline_world_16_9.jpg");
    slate!("offline_world_4_3.jpg");
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
            web: channelflow_plugin_api::PluginWeb::new(),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.display_mode, settings::DisplayMode::SlateImage);
        assert!(!plugin.enabled);
    }
}