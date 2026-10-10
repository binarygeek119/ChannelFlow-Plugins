//! The Local media source: files on this machine, no media server.
//!
//! Pick a folder and say what's in it (movies / TV / music / music videos).
//! Syncing walks the folder, reads the Jellyfin-format `.nfo` files for
//! metadata (https://jellyfin.org/docs/general/server/metadata/nfo/), and uses
//! `poster.jpg` / `poster.png` as the item poster when the NFO has none.

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
    Box::new(LocalPlugin::new())
}

/// The media-source handle the base registers alongside the plugin lifecycle.
pub fn media_source() -> Box<dyn MediaSource> {
    Box::new(LocalPlugin::new())
}

pub struct LocalPlugin {
    metadata: PluginManifest,
    logger: Option<PluginLogger>,
    enabled: bool,
}

impl LocalPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            logger: None,
            enabled: false,
        }
    }
}

impl Default for LocalPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Plugin for LocalPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("local.css", "text/css", include_bytes!("../web/local.css"));
        api.web.serve_embedded("local.html", "text/html", include_bytes!("../web/local.html"));
        api.web.serve_embedded("local.js", "text/javascript", include_bytes!("../web/local.js"));
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
impl MediaSource for LocalPlugin {
    fn type_id(&self) -> &'static str {
        "local"
    }

    fn display_name(&self) -> &'static str {
        "Local Files"
    }

    fn connection_fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::text("name", "Connection name").required(),
            FieldSpec::text("url", "Folder path").required(),
        ]
    }

    fn supported_media(&self) -> &[MediaType] {
        &[
            MediaType::Movie,
            MediaType::Series,
            MediaType::Artist,
            MediaType::Album,
            MediaType::MusicVideo,
        ]
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

    async fn list_libraries(&self, connection: &Connection, _api_key: &str) -> Vec<Library> {
        let kind = connection.media_kind.as_deref().unwrap_or("movies");
        vec![Library {
            remote_id: "local".to_string(),
            name: kind.to_string(),
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
        let library_name = ctx
            .enabled_libraries
            .first()
            .map(|library| library.name.clone())
            .unwrap_or_else(|| ctx.connection.media_kind.as_deref().unwrap_or("movies").to_string());
        let scanned = scanner::scan_folder(folder, &library_name);
        let mut items = Vec::with_capacity(scanned.len());
        for scanned_item in scanned {
            let mut item = scanned_item.catalog;
            if let Some(poster) = scanned_item.poster {
                if let Some(path) = write_poster(&ctx.image_root, &poster, &item.remote_id) {
                    item.poster_path = Some(path);
                }
            }
            report.added += 1;
            items.push(item);
        }
        match catalog.replace_library(ctx.connection_id, &library_name, items).await {
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(error = %error, "local: could not report the scan");
                report.errors += 1;
            }
        }
        report
    }
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect()
}

/// Copy a poster into `<images>/posters/Local/` and return the images-root
/// relative path the base catalog stores.
fn write_poster(image_root: &Path, source: &Path, remote_id: &str) -> Option<String> {
    let dir = image_root.join("posters").join("Local");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{}.jpg", sanitize_id(remote_id));
    std::fs::copy(source, dir.join(&name)).ok()?;
    Some(format!("posters/Local/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_declares_a_page() {
        let plugin = LocalPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.local");
        assert!(metadata.compatible_with("2.0.0"));
        assert!(metadata
            .ui_contributions
            .iter()
            .any(|contribution| matches!(contribution, UiContribution::Page { id, .. } if id == "local")));
    }

    #[test]
    fn local_source_identity_is_stable() {
        let source = LocalPlugin::new();
        assert_eq!(source.type_id(), "local");
        assert!(!source.connection_fields().is_empty());
    }
}