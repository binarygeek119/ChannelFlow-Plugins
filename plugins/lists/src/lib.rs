//! The Lists plugin.
//!
//! v1.0.0's list registry as a plugin: named collections of items you can
//! reuse across channels and presets. In v1 lists stood on Jellyfin playlists;
//! here each list is a named set of Media-catalog items stored in the plugin's
//! own namespaced storage.

mod routes;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth};
use channelflow_plugin_api::ui::UiContribution;

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(ListsPlugin::new())
}

pub struct ListsPlugin {
    metadata: PluginManifest,
    state: Option<Arc<ListsState>>,
    enabled: bool,
}

/// What the route handlers need: the plugin's own storage.
pub struct ListsState {
    pub storage: Arc<dyn channelflow_plugin_api::storage::PluginStorage>,
}

impl ListsPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<&Arc<ListsState>, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("the Lists plugin has not been loaded"))
    }
}

#[async_trait::async_trait]
impl Plugin for ListsPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("lists.css", "text/css", include_bytes!("../web/lists.css"));
        api.web.serve_embedded("lists.html", "text/html", include_bytes!("../web/lists.html"));
        api.web.serve_embedded("lists.js", "text/javascript", include_bytes!("../web/lists.js"));
        self.state = Some(Arc::new(ListsState {
            storage: api.storage.clone(),
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
        self.state()
            .ok()
            .map(|state| routes::router(state.storage.clone()))
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

impl Default for ListsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_declares_a_page() {
        let plugin = ListsPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.lists");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata
            .ui_contributions
            .iter()
            .any(|contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "lists")));
    }
}