//! The Scheduler plugin.
//!
//! Turns each channel's six layers into a generated broadcast day, using the AI
//! page's providers through the core. This phase ships the layer model, the
//! layer-6 override resolution, and the compile dry-run; the LLM build loop, the
//! horizon builder, the UI, and the playout writer land in later phases.

mod build;
mod horizon;
mod layers;
mod playout;
mod routes;
mod settings;
mod state;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth};
use channelflow_plugin_api::ui::UiContribution;
use tokio::sync::Mutex;

use crate::settings::{SchedulerSettings, SETTINGS_KEY};

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(SchedulerPlugin::new())
}

pub struct SchedulerPlugin {
    metadata: PluginManifest,
    state: Option<Arc<routes::RoutesState>>,
    enabled: bool,
}

impl SchedulerPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<routes::RoutesState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the Scheduler plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for SchedulerPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder: the core
        // serves it at /plugin/{id}/web/...
        api.web.serve_embedded("scheduler.css", "text/css", include_bytes!("../web/scheduler.css"));
        api.web.serve_embedded("scheduler.html", "text/html", include_bytes!("../web/scheduler.html"));
        api.web.serve_embedded("scheduler.js", "text/javascript", include_bytes!("../web/scheduler.js"));
        let settings = api
            .storage
            .get(SETTINGS_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|value| SchedulerSettings::parse(&value).ok())
            .unwrap_or_default();
        let horizon = Arc::new(horizon::Horizon::new(api.tasks.clone()));
        // `api.dir` is `<config>/plugins/{id}`; its grandparent is `<config>`,
        // where the playout folder lives.
        let config_root = api
            .dir
            .parent()
            .and_then(|plugins| plugins.parent())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| api.dir.clone());
        self.state = Some(Arc::new(routes::RoutesState {
            storage: api.storage.clone(),
            core: api.core.clone(),
            settings: Mutex::new(settings),
            horizon: horizon.clone(),
            tasks: api.tasks.clone(),
            config_root,
            logger: api.logger.clone(),
            rebuilding: std::sync::atomic::AtomicBool::new(false),
        }));

        // The horizon scheduler: every minute, at the configured build time,
        // build the horizon once for the day. The Tasks page stays base-owned;
        // this plugin owns its own loop and its own status route.
        if let Some(state) = self.state.clone() {
            tokio::spawn(async move {
                let mut last_date = String::new();
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    ticker.tick().await;
                    let settings = state.settings.lock().await.clone();
                    if !settings.enabled {
                        continue;
                    }
                    let now = chrono::Local::now();
                    let today = now.format("%Y-%m-%d").to_string();
                    if now.format("%H:%M").to_string() != settings.build_time {
                        continue;
                    }
                    if last_date == today {
                        continue;
                    }
                    last_date = today;
                    state
                        .horizon
                        .run(
                            &state.core,
                            &state.storage,
                            &state.config_root,
                            settings,
                            &state.logger,
                            "schedule",
                        )
                        .await;
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
        self.state = None;
    }

    async fn on_config(&mut self, _config: serde_json::Value) -> Result<(), PluginError> {
        Ok(())
    }

    fn routes(&self) -> Option<Router> {
        self.state().ok().map(|state| routes::router(state.clone()))
    }

    fn ui_contributions(&self) -> Vec<UiContribution> {
        self.metadata.ui_contributions.clone()
    }

    fn health(&self) -> PluginHealth {
        PluginHealth {
            ok: self.enabled,
            detail: if self.enabled { "enabled" } else { "disabled" }.to_string(),
        }
    }
}

impl Default for SchedulerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid() {
        let plugin = SchedulerPlugin::new();
        assert_eq!(plugin.metadata().id, "com.channelflow.scheduler");
        assert!(plugin.metadata().compatible_with("2.0.0"));
        assert!(plugin
            .metadata()
            .permissions
            .iter()
            .any(|permission| permission == "api:core:read"));
    }
}
