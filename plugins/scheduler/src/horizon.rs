//! The horizon builder: build `days_to_build` days for every enabled channel,
//! day-major, one build at a time. Progress is readable while it runs so the
//! UI can show "channel x/y, day d/N".

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use channelflow_plugin_api::core::CoreData;
use channelflow_plugin_api::plugin::PluginLogger;
use channelflow_plugin_api::storage::PluginStorage;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::build;
use crate::layers;
use crate::settings::{read_scopes, SchedulerSettings};
use crate::state;

/// What the horizon is doing, for `GET /build/status`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    /// `idle`, `running`, `done`, `done-with-errors`, or `failed`.
    pub state: String,
    pub channel_index: usize,
    pub channel_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    pub day_index: usize,
    pub day_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_day: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub built: usize,
    pub skipped: usize,
    pub degraded: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The one horizon builder the plugin owns. A single atomic flag keeps the
/// scheduled run and a manual run from overlapping.
pub struct Horizon {
    running: AtomicBool,
    progress: Mutex<Progress>,
}

impl Horizon {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                state: "idle".to_string(),
                ..Default::default()
            }),
        }
    }

    pub async fn snapshot(&self) -> Progress {
        self.progress.lock().await.clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Build the whole horizon. Returns at once when a build is already
    /// running, so one build happens at a time.
    pub async fn run(
        &self,
        core: &Arc<dyn CoreData>,
        storage: &Arc<dyn PluginStorage>,
        config_root: &Path,
        settings: SchedulerSettings,
        logger: &PluginLogger,
        trigger: &str,
    ) {
        if self.running.swap(true, Ordering::SeqCst) {
            logger.info("scheduler: a build is already running; skipping this trigger");
            return;
        }
        {
            let mut progress = self.progress.lock().await;
            *progress = Progress {
                state: "running".to_string(),
                day_total: settings.days_to_build as usize,
                started_at: Some(chrono::Local::now().to_rfc3339()),
                detail: Some(format!("triggered by {trigger}")),
                ..Default::default()
            };
        }

        let result = self.build_all(core, storage, config_root, &settings, logger).await;

        let mut progress = self.progress.lock().await;
        progress.finished_at = Some(chrono::Local::now().to_rfc3339());
        progress.state = match result {
            Ok(()) if progress.errors.is_empty() => "done".to_string(),
            Ok(()) => "done-with-errors".to_string(),
            Err(error) => {
                progress.errors.push(error);
                "failed".to_string()
            }
        };
        drop(progress);
        self.running.store(false, Ordering::SeqCst);
    }

    async fn build_all(
        &self,
        core: &Arc<dyn CoreData>,
        storage: &Arc<dyn PluginStorage>,
        config_root: &Path,
        settings: &SchedulerSettings,
        logger: &PluginLogger,
    ) -> Result<(), String> {
        let channels = core.channels().await.map_err(|error| error.to_string())?;
        let enabled: Vec<_> = channels.into_iter().filter(|channel| channel.enabled).collect();
        let today = chrono::Local::now().date_naive();
        {
            let mut progress = self.progress.lock().await;
            progress.channel_total = enabled.len();
        }

        for (index, channel) in enabled.iter().enumerate() {
            let scopes = read_scopes(storage.as_ref()).await;
            {
                let mut progress = self.progress.lock().await;
                progress.channel_index = index + 1;
                progress.channel_id = Some(channel.id.clone());
            }
            for day in 0..settings.days_to_build {
                let date_naive = today + chrono::Duration::days(day as i64);
                let date = date_naive.format("%Y-%m-%d").to_string();
                let weekday = chrono::Datelike::weekday(&date_naive).num_days_from_sunday();
                {
                    let mut progress = self.progress.lock().await;
                    progress.day_index = day as usize + 1;
                    progress.current_day = Some(date.clone());
                }

                let layers = state::read_channel_layers(storage.as_ref(), &channel.id).await;
                let resolved = layers::resolve(layers, &scopes, &channel.id, &date, weekday, &[]);
                let previous = state::read_day(storage.as_ref(), &channel.id, &date).await;

                match build::build_day(
                    core,
                    &channel.id,
                    &date,
                    resolved,
                    settings,
                    previous.as_ref(),
                )
                .await
                {
                    Ok(outcome) => {
                        if !outcome.skipped {
                            if let Err(error) =
                                state::store_day(storage.as_ref(), &channel.id, &outcome.day).await
                            {
                                let mut progress = self.progress.lock().await;
                                progress.errors.push(format!("{} {date}: {error}", channel.id));
                                continue;
                            }
                        }
                        if settings.write_playout {
                            let dir = crate::playout::channel_dir(
                                config_root,
                                &settings.playout_dir,
                                &channel.id,
                            );
                            if let Err(error) =
                                crate::playout::write(core, &outcome.day, &dir).await
                            {
                                let mut progress = self.progress.lock().await;
                                progress
                                    .errors
                                    .push(format!("{} {date} playout: {error}", channel.id));
                            }
                        }
                        let mut progress = self.progress.lock().await;
                        progress.provider = outcome.day.provider.clone();
                        if outcome.skipped {
                            progress.skipped += 1;
                        } else {
                            progress.built += 1;
                            if outcome.day.degraded {
                                progress.degraded += 1;
                            }
                        }
                    }
                    Err(error) => {
                        logger.warn(&format!(
                            "scheduler: {} {date} failed: {error}",
                            channel.id
                        ));
                        let mut progress = self.progress.lock().await;
                        progress.errors.push(format!("{} {date}: {error}", channel.id));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for Horizon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelflow_plugin_api::core::{
        CatalogFilter, CatalogRow, ChatRequest, ChatResponse, CoreChannel, CoreDataError,
    };
    use channelflow_plugin_api::storage::PluginStorageError;
    use serde_json::{json, Value};
    use std::collections::HashMap;

    /// A real in-memory store (the SDK's `InMemoryStorage` is read-only).
    #[derive(Default)]
    struct MemStorage {
        inner: std::sync::Mutex<HashMap<String, Value>>,
    }

    #[async_trait::async_trait]
    impl PluginStorage for MemStorage {
        async fn get(&self, key: &str) -> Result<Option<Value>, PluginStorageError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &Value) -> Result<(), PluginStorageError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.clone());
            Ok(())
        }
        async fn delete(&self, key: &str) -> Result<(), PluginStorageError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct FakeCore {
        channels: Vec<CoreChannel>,
        pool: Vec<CatalogRow>,
    }

    #[async_trait::async_trait]
    impl CoreData for FakeCore {
        async fn channels(&self) -> Result<Vec<CoreChannel>, CoreDataError> {
            Ok(self.channels.clone())
        }
        async fn installed_plugins(&self) -> Result<Vec<String>, CoreDataError> {
            Ok(Vec::new())
        }
        async fn catalog(&self, _filter: CatalogFilter) -> Result<Vec<CatalogRow>, CoreDataError> {
            Ok(self.pool.clone())
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, CoreDataError> {
            Ok(ChatResponse {
                content: json!({ "blocks": [
                    { "slot_index": 0, "span_slots": 48, "items": [ { "id": "a", "why": "test" } ] }
                ] })
                .to_string(),
                provider_id: "fake".to_string(),
                attempts: Vec::new(),
            })
        }
    }

    fn row() -> CatalogRow {
        CatalogRow {
            match_key: "a".into(),
            kind: "movie".into(),
            title: "A".into(),
            year: None,
            runtime_ms: Some(1_000),
            genres: vec![],
            official_rating: None,
            tags: vec![],
            series_key: None,
            season: None,
            episode: None,
        }
    }

    #[tokio::test]
    async fn the_horizon_builds_every_enabled_channel_day_major() {
        let core: Arc<dyn CoreData> = Arc::new(FakeCore {
            channels: vec![
                CoreChannel { id: "c1".into(), number: 1.0, name: "One".into(), enabled: true },
                CoreChannel { id: "c2".into(), number: 2.0, name: "Two".into(), enabled: false },
            ],
            pool: vec![row()],
        });
        let storage: Arc<dyn PluginStorage> = Arc::new(MemStorage::default());
        let horizon = Horizon::new();
        let settings = SchedulerSettings {
            days_to_build: 2,
            enabled: true,
            ..Default::default()
        };
        let logger = PluginLogger::new("com.channelflow.scheduler");

        horizon.run(&core, &storage, &std::env::temp_dir(), settings, &logger, "test").await;

        let progress = horizon.snapshot().await;
        assert_eq!(progress.state, "done");
        assert_eq!(progress.channel_total, 1, "only enabled channels are built");
        assert_eq!(progress.built, 2, "two days");
        assert_eq!(progress.degraded, 0);
        assert!(!horizon.is_running());

        // A second run finds the same input hashes and skips the model.
        let settings = SchedulerSettings {
            days_to_build: 2,
            enabled: true,
            ..Default::default()
        };
        horizon.run(&core, &storage, &std::env::temp_dir(), settings, &logger, "test").await;
        let progress = horizon.snapshot().await;
        assert_eq!(progress.skipped, 2, "an unchanged input is skipped");
        assert_eq!(progress.built, 0);
    }
}
