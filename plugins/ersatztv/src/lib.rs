//! The ErsatzTV Transcoding Engine plugin.
//!
//! Everything that used to be the Transcode page in the base moved here: the
//! instance defaults and each channel's overrides over them, all mirrored
//! exactly from next's `ffmpeg` + `normalization` schema. The Transcode screen
//! stays part of the base shell but talks only to routes this plugin mounts
//! under `/api/plugins/com.channelflow.ersatztv`.
//!
//! The instance defaults live in the plugin's own storage under `defaults`;
//! overrides live under `overrides`, a map of channel id to patch, so nothing
//! core-owned has to change when a channel's encoding is configured. Channels
//! themselves are read through the core-data handle (`api:core:read`).

mod api;
mod transcode;

use std::sync::Arc;

use axum::Router;
use channelflow_plugin_api::{
    core::CoreData,
    database::PluginDatabase,
    manifest::PluginManifest,
    plugin::PluginError,
    Plugin, PluginApi, PluginHealth, PluginLogger, PluginStorage, UiContribution,
};
use tokio::sync::Mutex;

use crate::transcode::TranscodeConfig;

/// The key the instance defaults are stored under.
pub const DEFAULTS_KEY: &str = "defaults";
/// The key the per-channel override map is stored under.
pub const OVERRIDES_KEY: &str = "overrides";

fn manifest() -> PluginManifest {
    PluginManifest::parse(include_str!("../plugin.json")).expect("bundled plugin.json is valid")
}

/// The one thing the base needs to construct this plugin.
pub fn plugin() -> Box<dyn Plugin> {
    Box::new(ErsatzTvPlugin::new())
}

pub struct ErsatzTvPlugin {
    metadata: PluginManifest,
    state: Option<Arc<ErsatzTvState>>,
    enabled: bool,
}

/// Shared runtime handed to the route handlers.
pub struct ErsatzTvState {
    pub defaults: Mutex<TranscodeConfig>,
    pub overrides: Mutex<serde_json::Map<String, serde_json::Value>>,
    pub storage: Arc<dyn PluginStorage>,
    pub core: Arc<dyn CoreData>,
    pub database: Arc<dyn PluginDatabase>,
    pub logger: PluginLogger,
    /// The plugin's own audit table in Postgres, when one exists.
    pub audit: Option<String>,
}

impl ErsatzTvPlugin {
    pub fn new() -> Self {
        Self {
            metadata: manifest(),
            state: None,
            enabled: false,
        }
    }

    fn state(&self) -> Result<Arc<ErsatzTvState>, PluginError> {
        self.state
            .clone()
            .ok_or_else(|| PluginError::new("ErsatzTV plugin has not been loaded"))
    }
}

impl ErsatzTvState {
    /// Record one change in the plugin's audit table. No-op when the base is
    /// on the file backend, where there is no table.
    async fn audit(
        &self,
        action: &str,
        channel_id: Option<&str>,
        payload: Option<&serde_json::Value>,
    ) {
        let Some(table) = &self.audit else {
            self.logger.warn("skipping audit write — no database");
            return;
        };
        let channel = channel_id
            .map(|id| format!("'{}'", quote(&id)))
            .unwrap_or_else(|| "NULL".to_string());
        let payload = payload
            .map(|value| format!("'{}'::jsonb", quote(&value.to_string())))
            .unwrap_or_else(|| "NULL".to_string());
        let sql = format!(
            "INSERT INTO {table} (action, channel_id, payload) VALUES ('{}', {channel}, {payload})",
            quote(action)
        );
        if let Err(error) = self.database.execute(&sql).await {
            self.logger.error(&format!("audit write failed: {error}"));
        }
    }

    async fn save_defaults(&self, config: &TranscodeConfig) -> Result<(), PluginError> {
        let value = serde_json::to_value(config)
            .map_err(|error| PluginError::new(error.to_string()))?;
        self.storage
            .set(DEFAULTS_KEY, &value)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?;
        *self.defaults.lock().await = config.clone();
        Ok(())
    }

    async fn overrides(&self) -> serde_json::Map<String, serde_json::Value> {
        self.overrides.lock().await.clone()
    }

    async fn save_overrides(
        &self,
        map: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), PluginError> {
        let value = serde_json::Value::Object(map.clone());
        self.storage
            .set(OVERRIDES_KEY, &value)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?;
        *self.overrides.lock().await = map;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Plugin for ErsatzTvPlugin {
    fn metadata(&self) -> &PluginManifest {
        &self.metadata
    }

    async fn on_load(&mut self, api: PluginApi) -> Result<(), PluginError> {
        // The web UI this plugin ships is hosted from its own folder:
        // the core serves it at /plugin/{id}/web/...
        api.web.serve_embedded("transcode.css", "text/css", include_bytes!("../web/transcode.css"));
        api.web.serve_embedded("transcode.html", "text/html", include_bytes!("../web/transcode.html"));
        api.web.serve_embedded("transcode.js", "text/javascript", include_bytes!("../web/transcode.js"));

        let defaults = match api
            .storage
            .get(DEFAULTS_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(value) => TranscodeConfig::parse(&value)
                .map_err(|error| PluginError::new(error.to_string()))?,
            None => TranscodeConfig::default(),
        };
        let overrides = match api
            .storage
            .get(OVERRIDES_KEY)
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
        {
            Some(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        // The plugin's own table, if the base gives it a database. Nothing
        // here fails the load: on the file backend there is no table and the
        // plugin simply stops keeping change history.
        let audit = match api.database.table_of("audit") {
            Some(table) => match api
                .database
                .create_table(
                    "audit",
                    "id BIGSERIAL PRIMARY KEY, at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                     action TEXT NOT NULL, channel_id TEXT, payload JSONB",
                )
                .await
            {
                Ok(()) => {
                    api.logger.info(&format!("audit table ready ({table})"));
                    Some(table)
                }
                Err(error) => {
                    api.logger.warn(&format!("no audit table: {error}"));
                    None
                }
            },
            None => None,
        };
        self.state = Some(Arc::new(ErsatzTvState {
            defaults: Mutex::new(defaults),
            overrides: Mutex::new(overrides),
            storage: api.storage,
            core: api.core,
            database: api.database.clone(),
            logger: api.logger.clone(),
            audit,
        }));
        api.logger.info("loaded");
        Ok(())
    }

    async fn on_enable(&mut self) -> Result<(), PluginError> {
        if self.enabled {
            return Ok(());
        }
        self.enabled = true;
        if let Some(state) = &self.state {
            let overrides = state.overrides().await;
            state.logger.info(&format!(
                "enabled with {} channel override(s)",
                overrides.len()
            ));
        }
        Ok(())
    }

    async fn on_disable(&mut self) -> Result<(), PluginError> {
        if !self.enabled {
            return Ok(());
        }
        self.enabled = false;
        if let Some(state) = &self.state {
            state.logger.info("disabled");
        }
        Ok(())
    }

    fn on_unload(&mut self) {
        self.state = None;
    }

    async fn on_config(&mut self, config: serde_json::Value) -> Result<(), PluginError> {
        let state = self.state()?;
        let parsed = TranscodeConfig::parse(&config)
            .map_err(|error| PluginError::new(error.to_string()))?;
        state.save_defaults(&parsed).await?;
        Ok(())
    }

    fn routes(&self) -> Option<Router> {
        self.state().ok().map(api::router)
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

/// Quote a string for inlining into SQL the plugin built itself (doubling the
/// quote is the whole escaping SQL needs for a literal).
pub(crate) fn quote(text: &str) -> String {
    text.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelflow_plugin_api::storage::InMemoryStorage;

    #[test]
    fn bundled_manifest_is_valid_and_compatible() {
        let plugin = ErsatzTvPlugin::new();
        let metadata = plugin.metadata();
        assert_eq!(metadata.id, "com.channelflow.ersatztv");
        assert!(metadata.compatible_with("2.0.0"));
    }

    #[tokio::test]
    async fn loads_defaults_from_storage() {
        let mut storage = InMemoryStorage::new();
        let defaults = TranscodeConfig::default();
        storage.seed(
            DEFAULTS_KEY,
            serde_json::to_value(&defaults).expect("serialize"),
        );

        let mut plugin = ErsatzTvPlugin::new();
        let api = PluginApi {
            id: metadata_id(),
            storage: Arc::new(storage),
            http: reqwest::Client::new(),
            base_version: "2.0.0".to_string(),
            dir: std::env::temp_dir(),
            logger: PluginLogger::new("com.channelflow.ersatztv"),
            core: Arc::new(channelflow_plugin_api::core::NoCoreData::default()),
            database: Arc::new(channelflow_plugin_api::database::NoPluginDatabase::default()),
            web: channelflow_plugin_api::PluginWeb::new(),
        };
        plugin.on_load(api).await.expect("load");
        let state = plugin.state().expect("state");
        assert_eq!(*state.defaults.lock().await, defaults);
        assert!(state.overrides().await.is_empty());

        plugin.on_enable().await.expect("enable");
        assert!(plugin.health().ok);
    }

    fn metadata_id() -> String {
        "com.channelflow.ersatztv".to_string()
    }
}