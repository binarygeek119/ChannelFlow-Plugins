//! The News plugin.
//!
//! 1.0.0's FlowWire newscast as a plugin: the header, article count, refresh
//! interval, TTS voice and engine, anchor intro/outro, the bulletin schedule,
//! the RSS feeds, and a headline preview. The settings live in the plugin's
//! own namespaced storage and the News tab in the nav is the `page`
//! contribution this plugin declares — install the plugin and the tab
//! appears.

mod api;
mod bulletin;
mod rewrite;
mod settings;
mod tts;

use std::sync::Arc;
use std::path::PathBuf;

use axum::Router;
use channelflow_plugin_api::contrib::PresetContribution;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::storage::PluginStorage;
use channelflow_plugin_api::ui::UiContribution;
use channelflow_plugin_api::{register_preset, unregister_preset};
use tokio::sync::Mutex;

use crate::settings::{NewsSettings, SETTINGS_KEY};

/// The id of the preset this plugin contributes to the Presets page. It is
/// registered on load, so FlowWire News is only in the preset list while this
/// plugin is installed.
const FLOWWIRE_PRESET_ID: &str = "channelflow-live-news";

fn flowwire_preset() -> PresetContribution {
    PresetContribution {
        id: FLOWWIRE_PRESET_ID.to_string(),
        plugin_id: "com.channelflow.news".to_string(),
        legacy: 125.0,
        subchannel: 119.4,
        name: "FlowWire News".to_string(),
        category: "News".to_string(),
        description: "Live RSS news channel with optional TTS".to_string(),
        logo: "/plugin/com.channelflow.news/web/logos/FlowWire.png".to_string(),
    }
}

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
    /// The last fetched newscast stories, keyed by the enabled feeds, with the
    /// instant they were fetched — v1.0.0 cached headlines for ~2 minutes.
    pub headline_cache: Mutex<Option<(String, std::time::Instant, Vec<serde_json::Value>)>>,
    /// The base config root (`<config>`), so the bulletin task can write its
    /// mp4s to `<config>/news` as 1.0.0 did.
    pub config_root: PathBuf,
    /// Read-only access to core data (channels, AI providers).
    pub core: Arc<dyn channelflow_plugin_api::core::CoreData>,
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
        // Offer itself to the Presets page: FlowWire News becomes a preset
        // (123.1 retro / 119.8 modern) while this plugin is installed.
        register_preset(flowwire_preset());
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("logos/FlowWire.png", "image/png", include_bytes!("../web/logos/FlowWire.png"));
        api.web.serve_embedded("media/Catherine_Wolfe.jpg", "image/jpeg", include_bytes!("../web/media/Catherine_Wolfe.jpg"));
        api.web.serve_embedded("audio/FlowWire_News/FlowWire-intro.mp3", "audio/mpeg", include_bytes!("../web/audio/FlowWire_News/FlowWire-intro.mp3"));
        api.web.serve_embedded("audio/FlowWire_News/FlowWire-outro.mp3", "audio/mpeg", include_bytes!("../web/audio/FlowWire_News/FlowWire-outro.mp3"));
        api.web.serve_embedded("news.css", "text/css", include_bytes!("../web/news.css"));
        api.web.serve_embedded("news.html", "text/html", include_bytes!("../web/news.html"));
        api.web.serve_embedded("news.js", "text/javascript", include_bytes!("../web/news.js"));

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
        // `api.dir` is `<config>/plugins/{id}`; its grandparent is `<config>`,
        // where the bulletin videos belong (v1.0.0 wrote `<config>/news`).
        let config_root = api
            .dir
            .parent()
            .and_then(|plugins| plugins.parent())
            .map(PathBuf::from)
            .unwrap_or_else(|| api.dir.clone());
        self.state = Some(Arc::new(NewsState {
            settings: Mutex::new(settings),
            storage: api.storage.clone(),
            http,
            logger: api.logger.clone(),
            headline_cache: Mutex::new(None),
            config_root,
            core: api.core.clone(),
        }));
        // The bulletin scheduler: every 30 seconds, fire at each configured
        // mark (once per mark), so the day's first build always runs and the
        // rest run only when enough stories are new.
        if let Some(state) = self.state.clone() {
            tokio::spawn(async move {
                let mut last_mark = String::new();
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
                loop {
                    ticker.tick().await;
                    let settings = state.settings.lock().await.clone();
                    if !settings.bulletin_enabled {
                        continue;
                    }
                    let now = chrono::Local::now();
                    if !bulletin::at_a_mark(&settings, now) {
                        continue;
                    }
                    let mark = now.format("%Y-%m-%d %H").to_string();
                    if mark == last_mark {
                        continue;
                    }
                    last_mark = mark;
                    let _ = bulletin::run(&state, &state.config_root.clone(), false).await;
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
        // Its preset disappears with it: FlowWire News is in the preset list
        // only while this plugin is installed.
        unregister_preset(FLOWWIRE_PRESET_ID);
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
            web: channelflow_plugin_api::PluginWeb::new(),
        };
        plugin.on_load(api).await.expect("load");
        let settings = plugin.state().expect("state").settings.lock().await.clone();
        assert_eq!(settings.header, "FlowWire News");
    }
}