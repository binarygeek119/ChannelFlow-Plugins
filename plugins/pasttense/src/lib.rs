//! The Past Tense News media source: files on this machine, no media server.
//!
//! The page browses to a folder of news *events*; each event folder becomes a
//! "Past Tense News" catalog item (a TV-show-style card, no seasons). Syncing
//! scans the event folders into the base media database — playback of the
//! coverage videos comes later.

mod routes;
mod scanner;

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::media::{
    Connection, FieldSpec, Library, MediaSource, MediaType, SyncCtx, SyncReport, TestResult,
};
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth, PluginLogger};
use channelflow_plugin_api::ui::UiContribution;

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(PastTensePlugin::new())
}

/// The media-source handle the base registers alongside the plugin lifecycle.
pub fn media_source() -> Box<dyn MediaSource> {
    Box::new(PastTensePlugin::new())
}

pub struct PastTensePlugin {
    metadata: PluginManifest,
    logger: Option<PluginLogger>,
    enabled: bool,
}

impl PastTensePlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            logger: None,
            enabled: false,
        }
    }
}

impl Default for PastTensePlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Plugin for PastTensePlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web
            .serve_embedded("pasttense.css", "text/css", include_bytes!("../web/pasttense.css"));
        api.web
            .serve_embedded("pasttense.html", "text/html", include_bytes!("../web/pasttense.html"));
        api.web
            .serve_embedded("pasttense.js", "text/javascript", include_bytes!("../web/pasttense.js"));
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

#[async_trait::async_trait]
impl MediaSource for PastTensePlugin {
    fn type_id(&self) -> &'static str {
        "news"
    }

    fn display_name(&self) -> &'static str {
        "Past Tense News"
    }

    fn connection_fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::text("name", "Connection name").required(),
            FieldSpec::text("url", "Events folder").required(),
        ]
    }

    fn supported_media(&self) -> &[MediaType] {
        // Each event folder is a show-like card; the coverage videos inside
        // are not catalogued individually (that comes with playback).
        &[MediaType::Series, MediaType::MusicVideo]
    }

    async fn test_connection(&self, connection: &Connection, _api_key: &str) -> TestResult {
        let path = connection.url.trim();
        if path.is_empty() {
            return TestResult::bad_url("enter a folder path");
        }
        if std::fs::metadata(path).map(|meta| meta.is_dir()).unwrap_or(false) {
            TestResult::ok("folder is readable")
        } else {
            TestResult::bad_url("not a readable folder on this machine")
        }
    }

    async fn list_libraries(&self, _connection: &Connection, _api_key: &str) -> Vec<Library> {
        vec![Library {
            remote_id: "news".to_string(),
            name: "news".to_string(),
            collection_type: None,
        }]
    }

    async fn sync_library(&self, ctx: SyncCtx) -> SyncReport {
        let mut report = SyncReport::default();
        let Some(catalog) = &ctx.catalog else {
            report.errors += 1;
            return report;
        };
        let folder = ctx.connection.url.trim();
        if folder.is_empty() {
            report.errors += 1;
            return report;
        }
        let scanned = scanner::scan_folder(folder);
        let mut items = Vec::with_capacity(scanned.len());
        for event in scanned {
            let mut item = event.catalog;
            if let Some(poster) = event.poster {
                if let Some(path) = write_poster(&ctx.image_root, &poster, &item.remote_id) {
                    item.poster_path = Some(path);
                }
            }
            report.added += 1;
            items.push(item);
        }
        match catalog.replace_library(ctx.connection_id, "news", items).await {
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(error = %error, "pasttense: could not report the scan");
                report.errors += 1;
            }
        }
        report
    }
}

/// Copy an event poster into `<images>/posters/News/` and return the
/// images-root relative path the base catalog stores.
fn write_poster(image_root: &Path, source: &Path, remote_id: &str) -> Option<String> {
    let dir = image_root.join("posters").join("News");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!(
        "{}.jpg",
        remote_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .take(64)
            .collect::<String>()
    );
    std::fs::copy(source, dir.join(&name)).ok()?;
    Some(format!("posters/News/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_declares_a_page() {
        let plugin = PastTensePlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.pasttense");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata
            .ui_contributions
            .iter()
            .any(|contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "pasttense")));
    }

    #[test]
    fn event_years_clean_from_folder_names() {
        assert_eq!(scanner::year_from("1992 LA Riots News coverage"), Some(1992));
        assert_eq!(scanner::year_from("Apollo 11 News coverage"), None);
        assert_eq!(scanner::clean_title("1992_LA_Riots_News_coverage"), "1992 LA Riots News coverage");
    }
}