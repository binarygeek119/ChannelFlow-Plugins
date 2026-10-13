//! One-off cleanup for the plugin's old media tables.
//!
//! The plugin used to keep its own copy of the media catalog — items, files,
//! streams, people, genres, the music hierarchy, sync runs and playout pins —
//! in `cf_com_channelflow_jellyfin_*` tables. The base's own catalog replaced
//! all of it, so on load the plugin drops what it left behind. Nothing but the
//! base's plugin key/value storage holds this plugin's settings now.

use std::sync::Arc;

use channelflow_plugin_api::database::PluginDatabase;

/// Every table the plugin used to own, by its unprefixed name.
const LEGACY_TABLES: &[&str] = &[
    "media_items",
    "media_files",
    "item_sources",
    "video_streams",
    "audio_streams",
    "subtitle_streams",
    "chapters",
    "people",
    "item_people",
    "genres",
    "studios",
    "tags",
    "item_genres",
    "item_studios",
    "item_tags",
    "provider_ids",
    "movies",
    "series",
    "seasons",
    "episodes",
    "artists",
    "albums",
    "tracks",
    "music_videos",
    "sync_runs",
    "playout_pins",
];

/// Drop the plugin's own media tables. A no-op on the file backend (there is
/// no plugin database there) and after the first run.
pub async fn drop_legacy_tables(db: &Arc<dyn PluginDatabase>) {
    let mut dropped = 0usize;
    for name in LEGACY_TABLES {
        let Some(table) = db.table_of(name) else {
            return;
        };
        match db.execute(&format!("DROP TABLE IF EXISTS {table} CASCADE")).await {
            Ok(_) => dropped += 1,
            Err(error) => {
                tracing::warn!(table = %table, error = %error.0, "jellyfin: could not drop a legacy table")
            }
        }
    }
    if dropped > 0 {
        tracing::info!(tables = dropped, "jellyfin: dropped the legacy media tables");
    }
}
