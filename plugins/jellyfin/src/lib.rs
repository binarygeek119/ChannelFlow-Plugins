//! The Jellyfin Media Source plugin.
//!
//! Registers as a media-source under the type id `jellyfin`. It lives the
//! ordinary [`Plugin`] lifecycle for routes and enable/disable, and also
//! implements the SDK's [`MediaSource`] contract, which is what the core's
//! connection forms and sync driver talk to. The exported
//! `channelflow_plugin_v1` symbol is the ABI entrypoint the future dynamic
//! loader will reach for (the SDK's `PLUGIN_ABI_VERSION` guards it).

mod client;
mod db;
mod dedup;
mod routes;
mod selection;
mod sync;

use std::sync::Arc;

use channelflow_plugin_api::database::PluginDatabase;
use channelflow_plugin_api::manifest::PluginManifest;
use channelflow_plugin_api::media::{
    Connection, FieldSpec, Library, MediaSource, MediaType, SyncCtx, SyncReport, TestResult,
};
use channelflow_plugin_api::plugin::{
    Plugin, PluginApi, PluginError, PluginHealth, PLUGIN_ABI_VERSION,
};

pub struct JellyfinPlugin {
    metadata: PluginManifest,
    enabled: bool,
    state: Option<PluginState>,
}

struct PluginState {
    db: Arc<dyn PluginDatabase>,
}

impl JellyfinPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            enabled: false,
            state: None,
        }
    }

    fn state(&self) -> Result<&PluginState, PluginError> {
        self.state
            .as_ref()
            .ok_or_else(|| PluginError::new("Jellyfin plugin has not been loaded"))
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

/// Called by the base after a `jellyfin` connection is deleted: the database
/// rows cascade first, this removes the poster files of items that lost every
/// source.
pub use sync::sweep_orphan_posters;

/// The ABI entrypoint: the base (or a future dynamic loader) calls this and
/// downcasts the `MediaSource` it gets back.
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn channelflow_plugin_v1() -> *mut dyn MediaSource {
    Box::into_raw(Box::new(JellyfinPlugin::new()))
}

#[no_mangle]
pub static channelflow_plugin_abi_version: u32 = PLUGIN_ABI_VERSION;

#[async_trait::async_trait]
impl Plugin for JellyfinPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // Tables initialise lazily on the first sync so a file-only install
        // (no Postgres) still loads and fails softly at sync time.
        self.state = Some(PluginState {
            db: api.database.clone(),
        });
        api.logger.info("loaded");
        Ok(())
    }

    async fn on_enable(&mut self) -> Result<(), PluginError> {
        if self.enabled {
            return Ok(());
        }
        self.enabled = true;
        Ok(())
    }

    async fn on_disable(&mut self) -> Result<(), PluginError> {
        if !self.enabled {
            return Ok(());
        }
        self.enabled = false;
        Ok(())
    }

    fn on_unload(&mut self) {
        self.state = None;
    }

    async fn on_config(&mut self, _config: serde_json::Value) -> Result<(), PluginError> {
        Ok(())
    }

    fn routes(&self) -> Option<axum::Router> {
        self.state()
            .ok()
            .map(|state| routes::router(state.db.clone()))
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
        let client = match client::JellyfinClient::new(connection, api_key) {
            Ok(client) => client,
            Err(_) => return TestResult::bad_url("Not a valid URL"),
        };
        match client.test().await {
            client::TestVerdict::Ok => TestResult::ok("Reachable, key accepted"),
            client::TestVerdict::AuthFailed => {
                if client.has_token() {
                    TestResult::auth_failed("Server reachable, API key rejected")
                } else {
                    TestResult::auth_failed("Server reachable, but no API key was given")
                }
            }
            client::TestVerdict::Unreachable => TestResult::unreachable("Could not reach the server"),
            client::TestVerdict::BadUrl => TestResult::bad_url("Unexpected response from that URL"),
        }
    }

    async fn list_libraries(&self, connection: &Connection, api_key: &str) -> Vec<Library> {
        match client::JellyfinClient::new(connection, api_key) {
            Ok(client) => client.libraries().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    async fn sync_library(&self, ctx: SyncCtx) -> SyncReport {
        match self.state() {
            Ok(state) => sync::run(&ctx, state.db.clone()).await,
            Err(error) => {
                tracing::error!(error = %error.0, "jellyfin: sync before load");
                SyncReport {
                    errors: 1,
                    ..SyncReport::default()
                }
            }
        }
    }

    fn routes(&self) -> Option<axum::Router> {
        self.state()
            .ok()
            .map(|state| routes::router(state.db.clone()))
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