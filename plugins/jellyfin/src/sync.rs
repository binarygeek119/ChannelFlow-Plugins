//! Library sync, in two phases, one media library type at a time.
//!
//! **Phase 1 — metadata and paths.** Every enabled library is walked in full
//! (moving on to the next library type when it finishes): items, sources,
//! files, streams, people and hierarchies are stored, and the base Media
//! catalog gets the titles. No images are fetched.
//!
//! **Phase 2 — images.** Every library is walked again: posters (for movies,
//! series and music videos) and every cast member's picture are fetched, saved
//! under `<config>/Images/`, and their paths written into the plugin's tables
//! and the base catalog.

use std::path::Path;
use std::sync::Arc;

use channelflow_plugin_api::database::PluginDatabase;
use channelflow_plugin_api::media::{CatalogItem, MediaCatalog, SyncCtx, SyncReport};
use channelflow_plugin_api::PluginError;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::client::JellyfinClient;
use crate::db::{ItemDetail, MediaDb};
use crate::dedup;

const PAGE: usize = 200;

/// A live snapshot of an in-flight sync, for the progress popup: the stage
/// (Movies / TV / Music), the count within the current library, and the
/// running totals.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncProgress {
    pub running: bool,
    /// `movies`, `tv`, or `music`.
    pub stage: String,
    /// Human label for the stage, e.g. "Movies".
    pub label: String,
    /// Items processed so far in the current library.
    pub current: u64,
    /// Items in the current library.
    pub total: u64,
    /// Libraries finished so far.
    pub done: u64,
    pub libraries: u64,
    pub added: u64,
    pub updated: u64,
    pub errors: u64,
}

impl Default for SyncProgress {
    fn default() -> Self {
        Self {
            running: false,
            stage: String::new(),
            label: String::new(),
            current: 0,
            total: 0,
            done: 0,
            libraries: 0,
            added: 0,
            updated: 0,
            errors: 0,
        }
    }
}

/// One process-wide progress slot, shared between the route that runs a sync
/// and the route the popup polls — the same pattern the plugin's loaded state
/// uses.
pub fn progress_state() -> &'static std::sync::Mutex<SyncProgress> {
    use std::sync::OnceLock;
    static PROGRESS: OnceLock<std::sync::Mutex<SyncProgress>> = OnceLock::new();
    PROGRESS.get_or_init(|| std::sync::Mutex::new(SyncProgress::default()))
}

/// Which stage a library belongs to, from Jellyfin's collection type.
pub fn stage_for(collection_type: Option<&str>) -> (&'static str, &'static str) {
    match collection_type.unwrap_or_default().to_ascii_lowercase().as_str() {
        "movies" | "folders" | "boxsets" => ("movies", "Movies"),
        "tvshows" => ("tv", "TV"),
        "music" | "musicvideos" => ("music", "Music"),
        _ => ("library", "Library"),
    }
}

/// Jellyfin's item type mapped to a base-catalog kind (the ones the Media page
/// lists), or `None` for items the catalog does not show.
fn catalog_kind(type_name: &str) -> Option<&'static str> {
    match type_name {
        "Movie" => Some("movie"),
        "Series" => Some("series"),
        "MusicAlbum" => Some("album"),
        "MusicArtist" => Some("artist"),
        "MusicVideo" => Some("musicvideo"),
        _ => None,
    }
}

/// A cross-source identity from Jellyfin's `ProviderIds`, so the base can match
/// the same media across sources (tmdb/imdb for video, MusicBrainz for music).
fn provider_match_id(raw: &Value) -> Option<String> {
    let providers = raw.get("ProviderIds")?;
    for key in [
        "Imdb",
        "Tmdb",
        "Tvdb",
        "MusicBrainzAlbum",
        "MusicBrainzArtist",
        "MusicBrainzReleaseGroup",
    ] {
        if let Some(value) = providers.get(key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(format!("{}:{}", key.to_ascii_lowercase(), value));
            }
        }
    }
    None
}

/// Which pass a `walk` is doing: either metadata (phase 1) or images (phase 2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Metadata,
    Images,
}

/// Fill the base media catalog for one library from a light top-level query.
/// Phase 1 (`with_images=false`) reports titles straight away; phase 2
/// (`with_images=true`) adds the posters as they are fetched.
async fn sync_catalog(
    client: &JellyfinClient,
    ctx: &SyncCtx,
    library: &channelflow_plugin_api::media::Library,
    catalog: &std::sync::Arc<dyn MediaCatalog>,
    with_images: bool,
) -> Result<(), String> {
    let mut items: Vec<CatalogItem> = Vec::new();
    for raw in client.catalog(&library.remote_id).await.map_err(|error| error.0)? {
        let Some(kind) = catalog_kind(raw.get("Type").and_then(Value::as_str).unwrap_or("")) else {
            continue;
        };
        let Some(remote) = raw.get("Id").and_then(Value::as_str) else {
            continue;
        };
        let title = raw.get("Name").and_then(Value::as_str).unwrap_or("");
        let year = raw.get("ProductionYear").and_then(Value::as_i64).map(|year| year as i32);
        let overview = raw.get("Overview").and_then(Value::as_str).map(str::to_string);
        items.push(
            CatalogItem::new(kind, remote, title)
                .year(year)
                .overview(overview)
                .library(&library.name)
                .match_id(provider_match_id(&raw)),
        );
    }

    if !with_images {
        catalog.replace_library(ctx.connection_id, &library.name, items).await?;
        return Ok(());
    }

    catalog
        .replace_library(ctx.connection_id, &library.name, items.clone())
        .await?;

    let mut waiting = 0usize;
    let mut last = std::time::Instant::now();
    for index in 0..items.len() {
        if matches!(items[index].kind.as_str(), "movie" | "series" | "musicvideo") {
            let remote = items[index].remote_id.clone();
            let kind = items[index].kind.clone();
            if let Ok(Some(path)) = write_poster(ctx, client, &remote, &kind, &remote).await {
                items[index].poster_path = Some(path);
                waiting += 1;
            }
        }
        if waiting > 0 && last.elapsed() >= std::time::Duration::from_secs(2) {
            catalog
                .replace_library(ctx.connection_id, &library.name, items.clone())
                .await?;
            waiting = 0;
            last = std::time::Instant::now();
        }
    }
    catalog
        .replace_library(ctx.connection_id, &library.name, items)
        .await?;
    Ok(())
}

pub async fn run(ctx: &SyncCtx, db: Arc<dyn PluginDatabase>) -> SyncReport {
    let progress = progress_state();
    {
        let mut state = progress.lock().unwrap();
        *state = SyncProgress::default();
        state.running = true;
        state.libraries = ctx.enabled_libraries.len() as u64;
    }

    let mut report = SyncReport::default();
    let media = MediaDb::new(db);
    if let Err(error) = media.init().await {
        tracing::error!(error = %error, "jellyfin: could not initialise tables");
        report.errors += 1;
        {
            let mut state = progress.lock().unwrap();
            state.running = false;
            state.errors = report.errors;
        }
        return report;
    }
    let client = match JellyfinClient::new(&ctx.connection, &ctx.api_key) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(error = %error, "jellyfin: bad connection settings");
            report.errors += 1;
            {
                let mut state = progress.lock().unwrap();
                state.running = false;
                state.errors = report.errors;
            }
            return report;
        }
    };
    let synced_at = chrono::Utc::now().to_rfc3339();

    // ---- Phase 1: metadata and paths, one library type at a time. ----------
    for library in &ctx.enabled_libraries {
        start_library(&progress, library);
        if let Some(catalog) = &ctx.catalog {
            if let Err(error) = sync_catalog(&client, ctx, library, catalog, false).await {
                tracing::warn!(library = %library.name, %error, "jellyfin: could not build the media catalog");
                report.errors += 1;
            }
        }
        walk(&client, ctx, &media, library, &synced_at, Phase::Metadata, &progress, &mut report).await;
        mark_done(&progress);
    }

    // ---- Phase 2: images (posters + people), one library type at a time. ---
    for library in &ctx.enabled_libraries {
        start_library(&progress, library);
        if let Some(catalog) = &ctx.catalog {
            if let Err(error) = sync_catalog(&client, ctx, library, catalog, true).await {
                tracing::warn!(library = %library.name, %error, "jellyfin: could not build the media catalog");
                report.errors += 1;
            }
        }
        walk(&client, ctx, &media, library, &synced_at, Phase::Images, &progress, &mut report).await;
        mark_done(&progress);
    }

    match media.mark_absent_missing(ctx.connection_id).await {
        Ok(removed) => report.removed = removed,
        Err(error) => {
            tracing::warn!(error = %error, "jellyfin: could not mark absent sources");
            report.errors += 1;
        }
    }
    {
        let mut state = progress.lock().unwrap();
        state.running = false;
        state.added = report.added;
        state.updated = report.updated;
        state.errors = report.errors;
    }
    report
}

fn start_library(progress: &'static std::sync::Mutex<SyncProgress>, library: &channelflow_plugin_api::media::Library) {
    let (stage, label) = stage_for(library.collection_type.as_deref());
    let mut state = progress.lock().unwrap();
    state.stage = stage.to_string();
    state.label = label.to_string();
    state.current = 0;
    state.total = 0;
}

fn mark_done(progress: &'static std::sync::Mutex<SyncProgress>) {
    let mut state = progress.lock().unwrap();
    state.done += 1;
}

async fn walk(
    client: &JellyfinClient,
    ctx: &SyncCtx,
    media: &MediaDb,
    library: &channelflow_plugin_api::media::Library,
    synced_at: &str,
    phase: Phase,
    progress: &'static std::sync::Mutex<SyncProgress>,
    report: &mut SyncReport,
) {
    let mut offset = 0usize;
    loop {
        let page = match client.items(&library.remote_id, offset).await {
            Ok(page) => page,
            Err(error) => {
                tracing::warn!(library = %library.name, %error, "jellyfin: library page failed");
                report.errors += 1;
                {
                    let mut state = progress.lock().unwrap();
                    state.errors = report.errors;
                }
                break;
            }
        };
        {
            let mut state = progress.lock().unwrap();
            state.total = page.total_record_count as u64;
        }
        for raw in &page.items {
            {
                let mut state = progress.lock().unwrap();
                state.current += 1;
            }
            let result = match phase {
                Phase::Metadata => metadata_upsert(media, ctx, raw, synced_at).await,
                Phase::Images => image_upsert(media, ctx, client, raw).await.map(|_| false),
            };
            match result {
                Ok(created) if phase == Phase::Metadata => {
                    if created {
                        report.added += 1;
                    } else {
                        report.updated += 1;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "jellyfin: item upsert failed");
                    report.errors += 1;
                }
            }
            {
                let mut state = progress.lock().unwrap();
                state.added = report.added;
                state.updated = report.updated;
                state.errors = report.errors;
            }
        }
        offset += PAGE;
        if offset >= page.total_record_count {
            break;
        }
    }
}

/// Phase 1: store an item's metadata and paths, without fetching any image.
async fn metadata_upsert(
    media: &MediaDb,
    ctx: &SyncCtx,
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

    let (item_id, created) = media
        .upsert_item_by_source(
            ctx.connection_id,
            jellyfin_id,
            &dedup_key,
            &media_type,
            &title,
            year,
            None,
            &ItemDetail::from_raw(raw),
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
        ensure_hierarchy(media, raw, item_id, &media_type, synced_at).await
    {
        tracing::warn!(error = %error.0, "jellyfin: could not build album/track hierarchy");
    }
    // Cast metadata: people rows + item links. Their images arrive in phase 2.
    if let Err(error) = media.upsert_people(item_id, raw).await {
        tracing::warn!(error = %error.0, "jellyfin: could not store item people");
    }
    // Genres and studios links, for the item detail page (best-effort).
    if let Err(error) = media.set_genres_and_studios(item_id, raw).await {
        tracing::warn!(error = %error.0, "jellyfin: could not store item genres/studios");
    }

    Ok(created)
}

/// Phase 2: fetch posters and cast pictures, save them under
/// `<config>/Images/`, and record the paths in the database.
async fn image_upsert(
    media: &MediaDb,
    ctx: &SyncCtx,
    client: &JellyfinClient,
    raw: &Value,
) -> Result<(), PluginError> {
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

    // Posters for the kinds the Media page shows.
    if matches!(media_type, "movie" | "series" | "musicvideo") {
        if let Some(path) = write_poster(ctx, client, jellyfin_id, &media_type, &dedup_key).await? {
            media
                .set_poster(ctx.connection_id, jellyfin_id, &path)
                .await
                .map_err(plugin_database)?;
        }
    }
    // Cast pictures, saved under <config>/Images/people/.
    if let Some(people) = raw.get("People").and_then(Value::as_array) {
        for person in people {
            let name = person.get("Name").and_then(Value::as_str).unwrap_or("").trim();
            if name.is_empty() {
                continue;
            }
            let person_id = person.get("Id").and_then(Value::as_str).unwrap_or("");
            if let Some(path) = write_people_image(ctx, client, person_id, name).await? {
                media
                    .set_people_image(name, &path)
                    .await
                    .map_err(plugin_database)?;
            }
        }
    }
    Ok(())
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
                return Ok(Some(store_path(&ctx.image_root, &path)));
            }
            _ => {}
        }
    }
    std::fs::create_dir_all(&dir)
        .map_err(|error| PluginError::new(format!("creating {dir:?}: {error}")))?;
    std::fs::write(&path, &bytes)
        .map_err(|error| PluginError::new(format!("writing {}: {error}", path.display())))?;
    Ok(Some(store_path(&ctx.image_root, &path)))
}

/// The form a stored image path takes: relative to the images root, as the
/// web UI's `/api/media/image` expects (`posters/Movies/x.jpg`), never a
/// host-dependent absolute or working-directory-relative path.
fn store_path(image_root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(image_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// A cast member's picture, saved under `<config>/Images/people/` keyed by the
/// person's Jellyfin id (or a sanitised name when no id is exposed).
async fn write_people_image(
    ctx: &SyncCtx,
    client: &JellyfinClient,
    person_id: &str,
    name: &str,
) -> Result<Option<String>, PluginError> {
    let bytes = match client.person_image(person_id, name).await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let file = if person_id.trim().is_empty() {
        sanitise_filename(name)
    } else {
        person_id.trim().to_string()
    };
    let dir = ctx.image_root.join("people");
    let path = dir.join(format!("{file}.jpg"));
    if path.exists() {
        match std::fs::read(&path) {
            Ok(existing) if sha256(&existing) == sha256(&bytes) => {
                return Ok(Some(store_path(&ctx.image_root, &path)));
            }
            _ => {}
        }
    }
    std::fs::create_dir_all(&dir)
        .map_err(|error| PluginError::new(format!("creating {dir:?}: {error}")))?;
    std::fs::write(&path, &bytes)
        .map_err(|error| PluginError::new(format!("writing {}: {error}", path.display())))?;
    Ok(Some(store_path(&ctx.image_root, &path)))
}

/// A filesystem-safe name fallback for a person without a Jellyfin id.
fn sanitise_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect()
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

    #[test]
    fn provider_ids_become_match_ids() {
        assert_eq!(
            provider_match_id(&serde_json::json!({ "ProviderIds": { "Imdb": "tt123" } })).as_deref(),
            Some("imdb:tt123")
        );
        assert_eq!(
            provider_match_id(&serde_json::json!({ "ProviderIds": { "Tmdb": "456" } })).as_deref(),
            Some("tmdb:456")
        );
        assert_eq!(provider_match_id(&serde_json::json!({})), None);
    }
}