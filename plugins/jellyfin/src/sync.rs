//! Library sync: page every enabled library, upsert a canonical item per
//! dedup key, restate each per-connection source, replace its files and
//! streams, write the poster only when it changed, then mark every source the
//! server no longer reports as missing.

use std::path::Path;
use std::sync::Arc;

use channelflow_plugin_api::database::PluginDatabase;
use channelflow_plugin_api::media::{SyncCtx, SyncReport};
use channelflow_plugin_api::PluginError;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::client::JellyfinClient;
use crate::db::MediaDb;
use crate::dedup;

const PAGE: usize = 200;

pub async fn run(ctx: &SyncCtx, db: Arc<dyn PluginDatabase>) -> SyncReport {
    let mut report = SyncReport::default();
    let media = MediaDb::new(db);
    if let Err(error) = media.init().await {
        tracing::error!(error = %error, "jellyfin: could not initialise tables");
        report.errors += 1;
        return report;
    }
    let client = match JellyfinClient::new(&ctx.connection, &ctx.api_key) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(error = %error, "jellyfin: bad connection settings");
            report.errors += 1;
            return report;
        }
    };
    let synced_at = chrono::Utc::now().to_rfc3339();

    for library in &ctx.enabled_libraries {
        let mut offset = 0usize;
        loop {
            let page = match client.items(&library.remote_id, offset).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(library = %library.name, %error, "jellyfin: library page failed");
                    report.errors += 1;
                    break;
                }
            };
            for raw in &page.items {
                match upsert(&media, ctx, &client, raw, &synced_at).await {
                    Ok(created) => {
                        if created {
                            report.added += 1;
                        } else {
                            report.updated += 1;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "jellyfin: item upsert failed");
                        report.errors += 1;
                    }
                }
            }
            offset += PAGE;
            if offset >= page.total_record_count {
                break;
            }
        }
    }

    match media.mark_absent_missing(ctx.connection_id).await {
        Ok(removed) => report.removed = removed,
        Err(error) => {
            tracing::warn!(error = %error, "jellyfin: could not mark absent sources");
            report.errors += 1;
        }
    }
    report
}

async fn upsert(
    media: &MediaDb,
    ctx: &SyncCtx,
    client: &JellyfinClient,
    raw: &Value,
    synced_at: &str,
) -> Result<bool, PluginError> {
    let jellyfin_id = raw
        .get("Id")
        .and_then(Value::as_str)
        .ok_or_else(|| PluginError::new("item has no Id"))?;
    let media_type = map_type(raw.get("Type").and_then(Value::as_str).unwrap_or(""));
    let title = raw.get("Name").and_then(Value::as_str).unwrap_or("").to_string();
    let year = raw.get("ProductionYear").and_then(Value::as_i64).map(|year| year as i32);

    let dedup_key = if media_type == "episode" {
        dedup::episode_key(
            raw.get("SeriesName").and_then(Value::as_str).unwrap_or(""),
            None,
            raw.get("ParentIndexNumber").and_then(Value::as_i64).unwrap_or(0) as i32,
            raw.get("IndexNumber").and_then(Value::as_i64).unwrap_or(0) as i32,
        )
    } else {
        dedup::key(&media_type, &title, year)
    };

    let poster_path = write_poster(ctx, client, jellyfin_id, &media_type, &dedup_key).await?;
    let (item_id, created) = media
        .upsert_item(&dedup_key, &media_type, &title, year, poster_path.as_deref(), synced_at)
        .await
        .map_err(plugin_database)?;
    let source_id = media
        .upsert_source(
            item_id,
            ctx.connection_id,
            jellyfin_id,
            raw.get("Path").and_then(Value::as_str),
            false,
            synced_at,
        )
        .await
        .map_err(plugin_database)?;
    media
        .replace_files(source_id, raw)
        .await
        .map_err(plugin_database)?;
    Ok(created)
}

fn map_type(raw: &str) -> &'static str {
    match raw {
        "Movie" => "movie",
        "Series" => "series",
        "Season" => "season",
        "Episode" => "episode",
        "Audio" => "track",
        "MusicVideo" => "musicvideo",
        _ => "unknown",
    }
}

/// Fetch the primary image and write it to
/// `<image_root>/posters/{group}/{dedup_key}.jpg`, but only when the bytes
/// changed — re-syncing must not churn unchanged posters. A 404 (no image)
/// leaves the old file (or none) alone.
async fn write_poster(
    ctx: &SyncCtx,
    client: &JellyfinClient,
    jellyfin_id: &str,
    media_type: &str,
    dedup_key: &str,
) -> Result<Option<String>, PluginError> {
    let bytes = match client.image(jellyfin_id, "Primary").await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let group = match media_type {
        "movie" => "Movies",
        "series" | "season" | "episode" => "TV",
        _ => "Music",
    };
    let dir = ctx.image_root.join("posters").join(group);
    let path = dir.join(format!("{dedup_key}.jpg"));
    if path.exists() {
        match std::fs::read(&path) {
            Ok(existing) if sha256(&existing) == sha256(&bytes) => {
                return Ok(Some(path.display().to_string()));
            }
            _ => {}
        }
    }
    std::fs::create_dir_all(&dir)
        .map_err(|error| PluginError::new(format!("creating {dir:?}: {error}")))?;
    std::fs::write(&path, &bytes)
        .map_err(|error| PluginError::new(format!("writing {}: {error}", path.display())))?;
    Ok(Some(path.display().to_string()))
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn plugin_database(error: channelflow_plugin_api::database::PluginDatabaseError) -> PluginError {
    PluginError::new(error.0)
}

/// Remove the poster files for items that lost every source (used after a
/// connection delete; the database rows cascade first). Returns the removed
/// paths. Called by the base, not from this crate's routes.
#[allow(dead_code)]
pub async fn sweep_orphan_posters(db: Arc<dyn PluginDatabase>) -> Vec<String> {
    let media = MediaDb::new(db);
    match media.orphan_sweep().await {
        Ok(posters) => {
            for path in &posters {
                let _ = std::fs::remove_file(Path::new(path));
            }
            posters
        }
        Err(error) => {
            tracing::warn!(error = %error, "jellyfin: orphan sweep failed");
            Vec::new()
        }
    }
}