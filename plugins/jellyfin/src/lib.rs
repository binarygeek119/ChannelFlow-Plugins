//! The Jellyfin Media Source plugin.
//!
//! Registers as a media-source under the type id `jellyfin`. It lives the
//! ordinary [`Plugin`] lifecycle for routes and enable/disable, and also
//! implements the SDK's [`MediaSource`] contract, which is what the core's
//! connection forms and sync driver talk to. The exported
//! `channelflow_plugin_v1` symbol is the ABI entrypoint the future dynamic
//! loader will reach for (the SDK's `PLUGIN_ABI_VERSION` guards it).
//!
//! The plugin keeps no media tables of its own: a sync reports everything —
//! items, files, cast and detail — into the base's catalog, which is what the
//! Media and People pages read. Its only persistent state is the settings the
//! base stores for it.

mod client;
mod migrate;
mod routes;
mod sync;

use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::media::{
    Connection, FieldSpec, Library, MediaSource, MediaType, SyncCtx, SyncReport, TestResult,
};
use channelflow_plugin_api::plugin::{Plugin, PluginApi, PluginError, PluginHealth};

pub struct JellyfinPlugin {
    metadata: PluginManifest,
    logger: Option<channelflow_plugin_api::plugin::PluginLogger>,
    enabled: bool,
}

impl JellyfinPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            logger: None,
            enabled: false,
        }
    }
}

/// The shipped `plugin.json` is the single source of truth for the manifest.
fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json"))
        .expect("bundled plugin.json is valid")
}

/// The one thing the compiled-in base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(JellyfinPlugin::new())
}

/// The media-source registration the core's registry takes, separate from the
/// lifecycle instance so the registry owns its own handle.
pub fn media_source() -> Box<dyn MediaSource> {
    Box::new(JellyfinPlugin::new())
}

/// The ABI entrypoint: a future dynamic loader calls this and downcasts the
/// `MediaSource` it gets back. Compiled only for the `abi` feature — the
/// standalone cdylib the release ships — because two plugins exporting the
/// same symbol collide when both are linked into the base binary.
#[cfg(feature = "abi")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn channelflow_plugin_v1() -> *mut dyn MediaSource {
    Box::into_raw(Box::new(JellyfinPlugin::new()))
}

#[cfg(feature = "abi")]
#[no_mangle]
pub static channelflow_plugin_abi_version: u32 =
    channelflow_plugin_api::plugin::PLUGIN_ABI_VERSION;

#[async_trait::async_trait]
impl Plugin for JellyfinPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("jellyfin.css", "text/css", include_bytes!("../web/jellyfin.css"));
        api.web.serve_embedded("jellyfin.html", "text/html", include_bytes!("../web/jellyfin.html"));
        api.web.serve_embedded("jellyfin.js", "text/javascript", include_bytes!("../web/jellyfin.js"));

        // The catalog is the base's now; drop the plugin's own media tables if
        // an older version left them behind.
        migrate::drop_legacy_tables(&api.database).await;
        api.logger.info("loaded");
        Ok(())
    }

    async fn on_enable(&mut self) -> Result<(), PluginError> {
        self.enabled = true;
        if let Some(logger) = &self.logger {
            logger.info("jellyfin: plugin enabled");
        }
        Ok(())
    }

    async fn on_disable(&mut self) -> Result<(), PluginError> {
        self.enabled = false;
        if let Some(logger) = &self.logger {
            logger.info("jellyfin: plugin disabled");
        }
        Ok(())
    }

    fn on_unload(&mut self) {}

    async fn on_config(&mut self, _config: serde_json::Value) -> Result<(), PluginError> {
        Ok(())
    }

    fn routes(&self) -> Option<axum::Router> {
        self.logger.as_ref().map(|logger| routes::router(logger.clone()))
    }

    fn ui_contributions(&self) -> Vec<channelflow_plugin_api::ui::UiContribution> {
        Vec::new()
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

#[async_trait::async_trait]
impl MediaSource for JellyfinPlugin {
    fn type_id(&self) -> &'static str {
        "jellyfin"
    }

    fn display_name(&self) -> &'static str {
        "Jellyfin"
    }

    fn connection_fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::text("name", "Connection name").required(),
            FieldSpec::text("url", "Server URL").required(),
            FieldSpec::secret("api_key", "API key"),
            FieldSpec::remaps("path_remaps", "Path remaps"),
            FieldSpec::action("test", "Test server"),
        ]
    }

    fn supported_media(&self) -> &[MediaType] {
        &[
            MediaType::Movie,
            MediaType::Series,
            MediaType::Season,
            MediaType::Episode,
            MediaType::Artist,
            MediaType::Album,
            MediaType::Track,
            MediaType::MusicVideo,
        ]
    }

    async fn test_connection(&self, connection: &Connection, api_key: &str) -> TestResult {
        if let Some(logger) = &self.logger {
            logger.info(&format!("jellyfin: testing connection to {}", connection.url));
        }
        
        let client = match client::JellyfinClient::new(connection, api_key) {
            Ok(client) => client,
            Err(_) => {
                if let Some(logger) = &self.logger {
                    logger.warn(&format!("jellyfin: invalid URL format for connection: {}", connection.url));
                }
                return TestResult::bad_url("Not a valid URL");
            }
        };
        
        match client.test().await {
            client::TestVerdict::Ok => {
                if let Some(logger) = &self.logger {
                    logger.info(&format!("jellyfin: connection test successful for {}", connection.url));
                }
                TestResult::ok("Reachable, key accepted")
            }
            client::TestVerdict::AuthFailed => {
                if let Some(logger) = &self.logger {
                    if client.has_token() {
                        logger.warn(&format!("jellyfin: authentication failed for {} - API key rejected", connection.url));
                        TestResult::auth_failed("Server reachable, API key rejected")
                    } else {
                        logger.warn(&format!("jellyfin: authentication failed for {} - no API key provided", connection.url));
                        TestResult::auth_failed("Server reachable, but no API key was given")
                    }
                } else if client.has_token() {
                    TestResult::auth_failed("Server reachable, API key rejected")
                } else {
                    TestResult::auth_failed("Server reachable, but no API key was given")
                }
            }
            client::TestVerdict::Unreachable => {
                if let Some(logger) = &self.logger {
                    logger.error(&format!("jellyfin: could not reach server at {}", connection.url));
                }
                TestResult::unreachable("Could not reach the server")
            }
            client::TestVerdict::BadUrl => {
                if let Some(logger) = &self.logger {
                    logger.warn(&format!("jellyfin: unexpected response from URL {}", connection.url));
                }
                TestResult::bad_url("Unexpected response from that URL")
            }
        }
    }

    async fn list_libraries(&self, connection: &Connection, api_key: &str) -> Vec<Library> {
        if let Some(logger) = &self.logger {
            logger.info(&format!("jellyfin: listing libraries from {}", connection.url));
        }
        
        match client::JellyfinClient::new(connection, api_key) {
            Ok(client) => {
                let libraries = client.libraries().await.unwrap_or_default();
                if let Some(logger) = &self.logger {
                    logger.info(&format!("jellyfin: found {} libraries", libraries.len()));
                }
                libraries
            }
            Err(error) => {
                if let Some(logger) = &self.logger {
                    logger.error(&format!("jellyfin: failed to create client for {}: {}", connection.url, error));
                }
                Vec::new()
            }
        }
    }

    async fn sync_library(&self, ctx: SyncCtx) -> SyncReport {
        if let Some(logger) = &self.logger {
            logger.info(&format!("jellyfin: starting library sync for connection '{}'", ctx.connection.name));
        }
        
        let report = sync::run(&ctx).await;
        
        if let Some(logger) = &self.logger {
            if report.added == 0 && report.updated == 0 && report.removed == 0 {
                logger.info(&format!(
                    "jellyfin: sync complete - no changes (total scanned: {})",
                    report.added + report.updated
                ));
            } else {
                logger.info(&format!(
                    "jellyfin: sync complete - added: {}, updated: {}, removed: {}",
                    report.added, report.updated, report.removed
                ));
            }
            
            if report.errors > 0 {
                logger.warn(&format!(
                    "jellyfin: sync completed with {} errors",
                    report.errors
                ));
            }
        }
        
        report
    }

    fn routes(&self) -> Option<axum::Router> {
        self.logger.as_ref().map(|logger| routes::router(logger.clone()))
    }

    async fn server_info(
        &self,
        connection: &Connection,
        api_key: &str,
    ) -> Option<serde_json::Value> {
        if let Some(logger) = &self.logger {
            logger.info(&format!("jellyfin: fetching server info from {}", connection.url));
        }
        
        let client = client::JellyfinClient::new(connection, api_key).ok()?;
        let (server_id, server_name) = client.system_info().await?;
        
        if let Some(logger) = &self.logger {
            logger.info(&format!("jellyfin: connected to server '{}' (id: {})", server_name, server_id));
        }
        
        Some(serde_json::json!({
            "server_id": server_id,
            "server_name": server_name,
        }))
    }

    fn item_web_url(&self, connection: &Connection, remote_id: &str) -> Option<String> {
        if remote_id.is_empty() {
            if let Some(logger) = &self.logger {
                logger.info("jellyfin: item_web_url called with empty remote_id");
            }
            return None;
        }
        
        let base = connection.url.trim().trim_end_matches('/');
        if base.is_empty() {
            if let Some(logger) = &self.logger {
                logger.info("jellyfin: item_web_url called with empty connection URL");
            }
            return None;
        }
        
        let mut url = format!("{base}/web/index.html#!/details?id={remote_id}");
        if let Some(server_id) = connection.server_id.as_deref().filter(|id| !id.is_empty()) {
            url.push_str(&format!("&serverId={server_id}"));
        }
        
        if let Some(logger) = &self.logger {
            logger.info(&format!("jellyfin: generated web URL for item {}: {}", remote_id, url));
        }
        
        Some(url)
    }
}

impl Default for JellyfinPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_compatible() {
        let plugin = JellyfinPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.jellyfin");
        assert!(metadata.compatible_with("2.0.0"), "must run on this base");
        assert!(metadata.permissions.iter().any(|p| p == "network:outbound"));
    }

    #[test]
    fn media_source_identity_is_stable() {
        let source = JellyfinPlugin::new();
        assert_eq!(source.type_id(), "jellyfin");
        assert_eq!(source.display_name(), "Jellyfin");
        assert!(!source.connection_fields().is_empty());
    }
}
