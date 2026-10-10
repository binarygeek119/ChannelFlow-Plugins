//! The Presets plugin.
//!
//! v1.0.0's Presets tab as a plugin: a ready-made Binarygeek119 lineup you can
//! stand up in one click. The plugin only *lists* the presets and which are
//! already covered (it reads core data); the page creates the missing channels
//! through the base's own channel API, so presets are a shortcut — never the
//! only way to add channels.

mod presets;
mod routes;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::core::CoreData;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth};
use channelflow_plugin_api::ui::UiContribution;

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(PresetsPlugin::new())
}

pub struct PresetsPlugin {
    metadata: PluginManifest,
    state: Option<Arc<PresetsState>>,
    enabled: bool,
}

/// What the route handlers need: read access to the core's channels.
pub struct PresetsState {
    pub core: Arc<dyn CoreData>,
}

impl PresetsPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<PresetsState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the Presets plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for PresetsPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("presets.css", "text/css", include_bytes!("../web/presets.css"));
        api.web.serve_embedded("presets.html", "text/html", include_bytes!("../web/presets.html"));
        api.web.serve_embedded("presets.js", "text/javascript", include_bytes!("../web/presets.js"));
        self.state = Some(Arc::new(PresetsState {
            core: api.core.clone(),
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
        self.state().ok().map(|state| routes::router(state.core.clone()))
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

impl Default for PresetsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_declares_a_page() {
        let plugin = PresetsPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.presets");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata
            .ui_contributions
            .iter()
            .any(|contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "presets")));
    }

    #[test]
    fn presets_have_unique_whole_numbers() {
        use std::collections::BTreeSet;
        let numbers: BTreeSet<u32> = presets::PRESETS.iter().map(|preset| preset.number).collect();
        assert_eq!(numbers.len(), presets::PRESETS.len(), "preset numbers must be unique");
        assert!(presets::PRESETS.iter().any(|preset| preset.number == 119));
    }
}