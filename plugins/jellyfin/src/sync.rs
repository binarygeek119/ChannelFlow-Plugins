//! Library sync, in two phases, one media library type at a time.
//!
//! **Phase 1 — metadata and paths.** Every enabled library is walked in full
//! (moving on to the next library type when it finishes) and reported to the
//! base's catalog: titles, files, cast and detail. No images are fetched.
//!
//! **Phase 2 — images.** Every library is walked again: posters (for movies,
//! series and music videos) and every cast member's picture are fetched, saved
//! under `<config>/Images/`, and their paths reported with the catalog entries.


use channelflow_plugin_api::media::{CatalogItem, MediaCatalog, SyncCtx, SyncReport};
use channelflow_plugin_api::PluginError;
use serde_json::Value;

use crate::client::JellyfinClient;


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
        "Audio" => Some("track"),
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

/// The strings of a JSON array field, e.g. `Genres` or `Tags`.
fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Build a base-catalog entry from one Jellyfin item, carrying the metadata
/// the Media page shows — runtime, genres, official rating, premiere date,
/// first-episode year (series), studios and tags — beside title/year/overview.
fn build_catalog_item(kind: &str, remote: &str, raw: &Value, library: &str) -> CatalogItem {
    let title = raw.get("Name").and_then(Value::as_str).unwrap_or("");
    let year = raw.get("ProductionYear").and_then(Value::as_i64).map(|year| year as i32);
    let overview = raw.get("Overview").and_then(Value::as_str).map(str::to_string);
    // Jellyfin reports runtime in ticks; one second is ten million of them.
    let runtime_seconds = raw.get("RunTimeTicks").and_then(Value::as_i64).map(|ticks| ticks / 10_000_000);
    let genres = strings(raw.get("Genres"));
    let official_rating = raw.get("OfficialRating").and_then(Value::as_str).map(str::to_string);
    // Jellyfin's PremiereDate is an ISO timestamp; keep the date part.
    let premiere_date = raw
        .get("PremiereDate")
        .and_then(Value::as_str)
        .map(|date| date.chars().take(10).collect::<String>());
    // For a series, ProductionYear is the year its first episode aired;
    // otherwise it carries the item's own year already.
    let first_episode_year = if kind == "series" { year } else { None };
    let studios = raw
        .get("Studios")
        .and_then(Value::as_array)
        .map(|studios| {
            studios
                .iter()
                .filter_map(|studio| studio.get("Name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let tags = strings(raw.get("Tags"));
    CatalogItem::new(kind, remote, title)
        .year(year)
        .overview(overview)
        .library(library)
        .match_id(provider_match_id(raw))
        .runtime_seconds(runtime_seconds)
        .genres(genres)
        .official_rating(official_rating)
        .premiere_date(premiere_date)
        .first_episode_year(first_episode_year)
        .studios(studios)
        .tags(tags)
        .detail(item_detail(raw))
        .people(item_people(raw))
        .files(catalog_files(raw))
}

/// The playable files a Jellyfin item reports, as the base's `CatalogFile`s —
/// the same `MediaSources` `item_detail` keeps, but first-class so the base
/// stores them in its own `media_files` table for scheduling.
fn catalog_files(raw: &Value) -> Vec<channelflow_plugin_api::media::CatalogFile> {
    raw.get("MediaSources")
        .and_then(Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .filter_map(|source| {
                    let path = source.get("Path").and_then(Value::as_str)?.trim();
                    if path.is_empty() {
                        return None;
                    }
                    Some(
                        channelflow_plugin_api::media::CatalogFile::new(path)
                            .remote_id(source.get("Id").and_then(Value::as_str).map(str::to_string))
                            .container(
                                source
                                    .get("Container")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                            )
                            .size_bytes(source.get("Size").and_then(Value::as_i64))
                            .runtime_seconds(
                                source
                                    .get("RunTimeTicks")
                                    .and_then(Value::as_i64)
                                    .map(|ticks| ticks / 10_000_000),
                            )
                            .streams(source.get("MediaStreams").cloned())
                            .chapters(source.get("Chapters").cloned()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The rich metadata a Jellyfin item carries beyond the summary fields, kept
/// in the base catalog as one JSON blob: tagline, ratings, container and
/// aspect, language, a music item's album/artist links, and the item's files
/// with their streams and chapters.
fn item_detail(raw: &Value) -> Option<Value> {
    let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_string);
    let mut detail = serde_json::Map::new();
    let mut put = |key: &str, value: Value| {
        if !value.is_null() {
            detail.insert(key.to_string(), value);
        }
    };
    put("tagline", raw.get("Taglines").and_then(Value::as_array).and_then(|lines| lines.first()).cloned().unwrap_or_else(|| text("Tagline").map(Value::String).unwrap_or(Value::Null)));
    put("community_rating", raw.get("CommunityRating").cloned().unwrap_or(Value::Null));
    put("critics_rating", raw.get("CriticRating").cloned().unwrap_or(Value::Null));
    put("custom_rating", text("CustomRating").map(Value::String).unwrap_or(Value::Null));
    put("container", text("Container").map(Value::String).unwrap_or(Value::Null));
    put("original_aspect_ratio", text("AspectRatio").map(Value::String).unwrap_or(Value::Null));
    put("language", text("PreferredMetadataLanguage").map(Value::String).unwrap_or(Value::Null));
    put("original_language", text("OriginalLanguage").map(Value::String).unwrap_or(Value::Null));
    // Music links: album → artist, track → album; the base music graph joins
    // on these instead of the plugin's own tables.
    put("album_id", text("AlbumId").map(Value::String).unwrap_or(Value::Null));
    put("album_artist", text("AlbumArtist").map(Value::String).unwrap_or(Value::Null));
    put("artist_id", text("ArtistId").map(Value::String).unwrap_or(Value::Null));
    put("series_id", text("SeriesId").map(Value::String).unwrap_or(Value::Null));
    // A music track's place, so the base can order an album's tracks.
    put("index_number", raw.get("IndexNumber").cloned().unwrap_or(Value::Null));
    put("parent_index_number", raw.get("ParentIndexNumber").cloned().unwrap_or(Value::Null));
    put("artists", raw.get("Artists").cloned().unwrap_or(Value::Null));

    // Files with their streams and chapters (the item's playable sources).
    let files: Vec<Value> = raw
        .get("MediaSources")
        .and_then(Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .map(|source| {
                    serde_json::json!({
                        "remote_id": source.get("Id"),
                        "container": source.get("Container"),
                        "path": source.get("Path"),
                        "size_bytes": source.get("Size"),
                        "bitrate": source.get("Bitrate"),
                        "runtime_ticks": source.get("RunTimeTicks"),
                        "media_streams": source.get("MediaStreams").cloned().unwrap_or(Value::Null),
                        "chapters": source.get("Chapters").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if !files.is_empty() {
        put("files", Value::Array(files));
    }
    if raw.get("Chapters").is_some() {
        put("chapters", raw.get("Chapters").cloned().unwrap_or(Value::Null));
    }

    if detail.is_empty() {
        None
    } else {
        Some(Value::Object(detail))
    }
}

/// The cast and crew a Jellyfin item reports.
fn item_people(raw: &Value) -> Vec<channelflow_plugin_api::media::PersonCredit> {
    raw.get("People")
        .and_then(Value::as_array)
        .map(|people| {
            people
                .iter()
                .filter_map(|person| {
                    let name = person.get("Name").and_then(Value::as_str)?.trim();
                    if name.is_empty() {
                        return None;
                    }
                    Some(channelflow_plugin_api::media::PersonCredit {
                        name: name.to_string(),
                        role_type: person.get("Type").and_then(Value::as_str).unwrap_or("").to_string(),
                        role: person.get("Role").and_then(Value::as_str).unwrap_or("").to_string(),
                        character: person.get("Role").and_then(Value::as_str).unwrap_or("").to_string(),
                        image_path: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Which pass a `walk` is doing: either metadata (phase 1) or images (phase 2).
async fn sync_catalog(
    client: &JellyfinClient,
    ctx: &SyncCtx,
    library: &channelflow_plugin_api::media::Library,
    catalog: &std::sync::Arc<dyn MediaCatalog>,
    with_images: bool,
) -> Result<usize, String> {
    let raws = client
        .catalog(&library.remote_id)
        .await
        .map_err(|error| error.0)?;
    let mut items: Vec<CatalogItem> = Vec::new();
    // Which raw item each catalog entry came from, for its cast photos.
    let mut origins: Vec<usize> = Vec::new();
    for (index, raw) in raws.iter().enumerate() {
        let Some(kind) = catalog_kind(raw.get("Type").and_then(Value::as_str).unwrap_or("")) else {
            continue;
        };
        let Some(remote) = raw.get("Id").and_then(Value::as_str) else {
            continue;
        };
        items.push(build_catalog_item(kind, remote, raw, &library.name));
        origins.push(index);
    }

    if !with_images {
        let count = items.len();
        catalog
            .replace_library(ctx.connection_id, &library.name, items)
            .await?;
        return Ok(count);
    }

    catalog
        .replace_library(ctx.connection_id, &library.name, items.clone())
        .await?;

    // Cast pictures, fetched once per name: the People page shows them, and a
    // music artist with no primary image borrows their own.
    let mut people_images: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    let mut waiting = 0usize;
    let mut last = std::time::Instant::now();
    for index in 0..items.len() {
        let raw = &raws[origins[index]];
        for credit in items[index].people.iter_mut() {
            if let Some(cached) = people_images.get(&credit.name) {
                credit.image_path = cached.clone();
                continue;
            }
            let person_id = raw
                .get("People")
                .and_then(Value::as_array)
                .and_then(|people| {
                    people.iter().find(|person| {
                        person.get("Name").and_then(Value::as_str).map(str::trim)
                            == Some(credit.name.as_str())
                    })
                })
                .and_then(|person| person.get("Id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let path = write_people_image(ctx, client, person_id, &credit.name)
                .await
                .ok()
                .flatten();
            people_images.insert(credit.name.clone(), path.clone());
            credit.image_path = path;
            if credit.image_path.is_some() {
                waiting += 1;
            }
        }

        // Posters the Media page shows: movies, shows, music videos, and — the
        // music tab's — album covers and artist images.
        if matches!(
            items[index].kind.as_str(),
            "movie" | "series" | "musicvideo" | "album" | "artist"
        ) {
            let remote = items[index].remote_id.clone();
            let kind = items[index].kind.clone();
            if let Ok(Some(path)) = write_poster(ctx, client, &remote, &kind, &remote).await {
                items[index].poster_path = Some(path);
                waiting += 1;
            } else if matches!(kind.as_str(), "album" | "artist") {
                // No primary image on the server (soundtrack composers usually
                // have none) — fall back to the synced person photo when the
                // same name has one, so music artists still show an image.
                if let Some(Some(path)) = people_images.get(&items[index].title) {
                    items[index].poster_path = Some(path.clone());
                    waiting += 1;
                }
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
    let count = items.len();
    catalog
        .replace_library(ctx.connection_id, &library.name, items)
        .await?;
    Ok(count)
}

pub async fn run(ctx: &SyncCtx) -> SyncReport {
    let progress = progress_state();
    {
        let mut state = progress.lock().unwrap();
        *state = SyncProgress::default();
        state.running = true;
        state.libraries = ctx.enabled_libraries.len() as u64;
    }

    let mut report = SyncReport::default();
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
    // The catalog handle is how this sync reaches the base: without it there is
    // nowhere to write. The base's task always passes one.
    let Some(catalog) = &ctx.catalog else {
        tracing::warn!("jellyfin: sync without a catalog handle");
        report.errors += 1;
        {
            let mut state = progress.lock().unwrap();
            state.running = false;
            state.errors = report.errors;
        }
        return report;
    };

    // ---- Phase 1: metadata and paths, one library type at a time. ----------
    for library in &ctx.enabled_libraries {
        start_library(&progress, library);
        match sync_catalog(&client, ctx, library, catalog, false).await {
            Ok(count) => report.added += count as u64,
            Err(error) => {
                tracing::warn!(library = %library.name, %error, "jellyfin: could not build the media catalog");
                report.errors += 1;
            }
        }
        mark_done(&progress);
    }

    // ---- Phase 2: images (posters + cast), one library type at a time. -----
    for library in &ctx.enabled_libraries {
        start_library(&progress, library);
        if let Err(error) = sync_catalog(&client, ctx, library, catalog, true).await {
            tracing::warn!(library = %library.name, %error, "jellyfin: could not build the media catalog");
            report.errors += 1;
        }
        mark_done(&progress);
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

/// Fetch the primary image and write it to
/// `<image_root>/posters/{group}/{dedup_key}.jpg`. A poster already on disk is
/// kept as it is: re-downloading every image on every pass is what made a full
/// sync crawl, and a poster that changed on the server is picked up once its
/// file — or the images directory — is removed. A 404 (no image) leaves the old
/// file (or none) alone.
async fn write_poster(
    ctx: &SyncCtx,
    client: &JellyfinClient,
    jellyfin_id: &str,
    media_type: &str,
    dedup_key: &str,
) -> Result<Option<String>, PluginError> {
    let group = match media_type {
        "movie" => "Movies",
        "series" | "season" | "episode" => "TV",
        _ => "Music",
    };
    let dir = ctx.image_root.join("posters").join(group);
    let path = dir.join(format!("{dedup_key}.jpg"));
    if path.exists() {
        return Ok(Some(store_path(&ctx.image_root, &path)));
    }
    let bytes = match client.image(jellyfin_id, "Primary").await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
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
/// person's Jellyfin id (or a sanitised name when no id is exposed). Like a
/// poster, an image already on disk is kept as it is.
async fn write_people_image(
    ctx: &SyncCtx,
    client: &JellyfinClient,
    person_id: &str,
    name: &str,
) -> Result<Option<String>, PluginError> {
    let file = if person_id.trim().is_empty() {
        sanitise_filename(name)
    } else {
        person_id.trim().to_string()
    };
    let dir = ctx.image_root.join("people");
    let path = dir.join(format!("{file}.jpg"));
    if path.exists() {
        return Ok(Some(store_path(&ctx.image_root, &path)));
    }
    let bytes = match client.person_image(person_id, name).await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn catalog_items_carry_runtime_genres_rating_premiere_studios_tags() {
        let raw = serde_json::json!({
            "Name": "Groundhog Day",
            "ProductionYear": 1993,
            "Overview": "A weatherman relives the same day.",
            "RunTimeTicks": 608_400_000_000i64,
            "Genres": ["Comedy", "Fantasy"],
            "OfficialRating": "PG",
            "PremiereDate": "1993-02-12T00:00:00.0000000Z",
            "Studios": [{ "Name": "Columbia Pictures" }, { "Name": "No Name" }],
            "Tags": ["time loop", "classic"]
        });
        let item = build_catalog_item("movie", "abc", &raw, "Movies");
        assert_eq!(item.year, Some(1993));
        assert_eq!(item.runtime_seconds, Some(60_840), "ticks become seconds");
        assert_eq!(item.genres, ["Comedy", "Fantasy"]);
        assert_eq!(item.official_rating.as_deref(), Some("PG"));
        assert_eq!(item.premiere_date.as_deref(), Some("1993-02-12"));
        assert_eq!(item.studios, ["Columbia Pictures", "No Name"]);
        assert_eq!(item.tags, ["time loop", "classic"]);
        assert_eq!(item.first_episode_year, None, "a movie has no first-episode year");
    }

    #[test]
    fn a_series_first_episode_year_is_its_production_year() {
        let raw = serde_json::json!({ "Name": "Frasier", "ProductionYear": 1993 });
        let item = build_catalog_item("series", "s1", &raw, "TV");
        assert_eq!(item.first_episode_year, Some(1993));
    }
}