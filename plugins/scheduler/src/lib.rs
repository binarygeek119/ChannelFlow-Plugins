//! The Scheduler plugin.
//!
//! Turns each channel's six layers into a generated broadcast day, using the AI
//! page's providers through the core. This phase ships the layer model, the
//! layer-6 override resolution, and the compile dry-run; the LLM build loop, the
//! horizon builder, the UI, and the playout writer land in later phases.

mod build;
mod layers;
mod routes;
mod settings;

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
        let settings = api
            .storage
            .get(SETTINGS_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|value| SchedulerSettings::parse(&value).ok())
            .unwrap_or_default();
        self.state = Some(Arc::new(routes::RoutesState {
            storage: api.storage.clone(),
            core: api.core.clone(),
            settings: Mutex::new(settings),
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
