//! The Holiday Channel plugin.
//!
//! v1.0.0's Holiday Channel as a plugin: the seasonal calendar (which
//! holiday's window the date falls in, Easter and US Thanksgiving computed),
//! the period logos, and a lineup of the catalog items that match the active
//! holiday. The matching logic is ported straight from v1.0.0's
//! `HolidayChannelCalendar`; the page supplies the catalog rows, which only
//! the logged-in front end can fetch.

mod calendar;
mod routes;

use axum::Router;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::ui::UiContribution;

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(HolidayChannelPlugin::new())
}

pub struct HolidayChannelPlugin {
    metadata: PluginManifest,
    logger: Option<PluginLogger>,
    enabled: bool,
}

impl HolidayChannelPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            logger: None,
            enabled: false,
        }
    }
}

#[async_trait::async_trait]
impl Plugin for HolidayChannelPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web
            .serve_embedded("holiday.css", "text/css", include_bytes!("../web/holiday.css"));
        api.web
            .serve_embedded("holiday.html", "text/html", include_bytes!("../web/holiday.html"));
        api.web
            .serve_embedded("holiday.js", "text/javascript", include_bytes!("../web/holiday.js"));
        // The period logos carried over from v1.0.0's HolidayChannelService.
        api.web
            .serve_embedded("logos/off-season.png", "image/png", include_bytes!("../web/logos/off-season.png"));
        api.web
            .serve_embedded("logos/valentines.png", "image/png", include_bytes!("../web/logos/valentines.png"));
        api.web
            .serve_embedded("logos/easter.png", "image/png", include_bytes!("../web/logos/easter.png"));
        api.web
            .serve_embedded("logos/independence.png", "image/png", include_bytes!("../web/logos/independence.png"));
        api.web
            .serve_embedded("logos/halloween.png", "image/png", include_bytes!("../web/logos/halloween.png"));
        api.web
            .serve_embedded("logos/October.png", "image/png", include_bytes!("../web/logos/October.png"));
        api.web
            .serve_embedded("logos/thanksgiving.png", "image/png", include_bytes!("../web/logos/thanksgiving.png"));
        api.web
            .serve_embedded("logos/christmas.png", "image/png", include_bytes!("../web/logos/christmas.png"));
        self.logger = Some(api.logger.clone());
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
        self.logger = None;
    }

    async fn on_config(&mut self, _config: serde_json::Value) -> Result<(), PluginError> {
        Ok(())
    }

    fn routes(&self) -> Option<Router> {
        Some(routes::router())
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

impl Default for HolidayChannelPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_declares_a_page() {
        let plugin = HolidayChannelPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.holidaychannel");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata
            .ui_contributions
            .iter()
            .any(|contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "holidaychannel")));
    }
}