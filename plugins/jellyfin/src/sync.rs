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
        .upsert_item_by_source(
            ctx.connection_id,
            jellyfin_id,
            &dedup_key,
            &media_type,
            &title,
            year,
            poster_path.as_deref(),
            synced_at,
        )
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

    // Jellyfin nests every track under an album; build the album → artist
    // nodes and link this track into them (best-effort — a lone track stays
    // ungrouped rather than failing the sync).
    if let Err(error) =
        ensure_hierarchy(&media, raw, item_id, &media_type, synced_at).await
    {
        tracing::warn!(error = %error.0, "jellyfin: could not build album/track hierarchy");
    }
    Ok(created)
}

/// Jellyfin returns `Audio` items (tracks) with their `Album`, `AlbumArtist`
/// and disc/track numbers, but never the album/artist items themselves —
/// those are built here as synthesised grouping nodes so the library gets a
/// real album → track (and artist) hierarchy instead of a flat track list.
async fn ensure_hierarchy(
    media: &MediaDb,
    raw: &Value,
    item_id: i64,
    media_type: &str,
    synced_at: &str,
) -> Result<(), PluginError> {
    match media_type {
        "track" => {
            let Some(album_title) = album_title(raw) else {
                return Ok(());
            };
            let year = raw
                .get("ProductionYear")
                .and_then(Value::as_i64)
                .map(|year| year as i32);
            let artist_item = match album_artist(raw) {
                Some(artist) => {
                    let (artist_id, _) = media
                        .upsert_grouping(
                            &dedup::key("artist", &artist, None),
                            "artist",
                            &artist,
                            None,
                            synced_at,
                        )
                        .await
                        .map_err(plugin_database)?;
                    media.ensure_artist_row(artist_id).await.map_err(plugin_database)?;
                    Some(artist_id)
                }
                None => None,
            };
            let (album_id, _) = media
                .upsert_grouping(
                    &dedup::key("album", &album_title, year),
                    "album",
                    &album_title,
                    year,
                    synced_at,
                )
                .await
                .map_err(plugin_database)?;
            media.ensure_album_row(album_id, artist_item).await.map_err(plugin_database)?;
            let (track_number, disc_number) = track_numbers(raw);
            media
                .link_track(item_id, album_id, track_number, disc_number)
                .await
                .map_err(plugin_database)?;
            Ok(())
        }
        "musicvideo" => {
            if let Some(artist) = album_artist(raw) {
                let (artist_id, _) = media
                    .upsert_grouping(
                        &dedup::key("artist", &artist, None),
                        "artist",
                        &artist,
                        None,
                        synced_at,
                    )
                    .await
                    .map_err(plugin_database)?;
                media.ensure_artist_row(artist_id).await.map_err(plugin_database)?;
                media.link_music_video(item_id, Some(artist_id)).await.map_err(plugin_database)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The album a track belongs to, from the track's own fields.
fn album_title(raw: &Value) -> Option<String> {
    let title = raw.get("Album").and_then(Value::as_str).unwrap_or("").trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// The artist for an album/track/music-video, in Jellyfin's field order.
fn album_artist(raw: &Value) -> Option<String> {
    let direct = raw.get("AlbumArtist").and_then(Value::as_str);
    let from_album_artists = raw
        .get("AlbumArtists")
        .and_then(Value::as_array)
        .and_then(|artists| artists.first())
        .and_then(|artist| artist.get("Name"))
        .and_then(Value::as_str);
    let from_artist_items = raw
        .get("ArtistItems")
        .and_then(Value::as_array)
        .and_then(|artists| artists.first())
        .and_then(|artist| artist.get("Name"))
        .and_then(Value::as_str);
    [direct, from_album_artists, from_artist_items]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

/// `(track_number, disc_number)` — Jellyfin's `IndexNumber` /
/// `ParentIndexNumber`.
fn track_numbers(raw: &Value) -> (Option<i32>, Option<i32>) {
    (
        raw.get("IndexNumber").and_then(Value::as_i64).map(|n| n as i32),
        raw.get("ParentIndexNumber").and_then(Value::as_i64).map(|n| n as i32),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_and_artist_come_from_the_track_fields() {
        let raw = serde_json::json!({
            "Album": "Random Access Memories",
            "AlbumArtist": "Daft Punk",
            "AlbumArtists": [{"Name": "Daft Punk", "Id": "a1"}],
            "ArtistItems": [{"Name": "Someone Else", "Id": "x"}],
        });
        assert_eq!(album_title(&raw).as_deref(), Some("Random Access Memories"));
        assert_eq!(album_artist(&raw).as_deref(), Some("Daft Punk"));
    }

    #[test]
    fn artist_falls_back_to_album_artists_then_artist_items() {
        let raw = serde_json::json!({
            "Album": "Greatest Hits",
            "AlbumArtists": [{"Name": "The Band"}],
        });
        assert_eq!(album_artist(&raw).as_deref(), Some("The Band"));

        let raw = serde_json::json!({
            "Album": "Live",
            "ArtistItems": [{"Name": "Live Artist"}],
        });
        assert_eq!(album_artist(&raw).as_deref(), Some("Live Artist"));
    }

    #[test]
    fn missing_album_means_no_grouping() {
        let raw = serde_json::json!({ "Name": "Lone Track" });
        assert_eq!(album_title(&raw), None);
        assert_eq!(album_artist(&raw), None);
    }

    #[test]
    fn disc_and_track_numbers_are_extracted() {
        let raw = serde_json::json!({
            "IndexNumber": 7,
            "ParentIndexNumber": 2,
        });
        assert_eq!(track_numbers(&raw), (Some(7), Some(2)));
        assert_eq!(track_numbers(&serde_json::json!({})), (None, None));
    }
}