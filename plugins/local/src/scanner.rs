//! Folder scanning for the Local media source.
//!
//! Walks a folder and turns Jellyfin-style `.nfo` files plus
//! `poster.jpg`/`poster.png` into base-catalog items.
//!
//! NFO layout (https://jellyfin.org/docs/general/server/metadata/nfo/):
//!   * a movie file with a sibling `Movie (2000).nfo` -> one movie row,
//!   * a show folder with `tvshow.nfo` -> one series row,
//!   * `artist.nfo` / `album.nfo` in music folders -> artist/album rows,
//!   * any of those carry `<title>`, `<year>`, `<plot>`, and
//!     `<uniqueid type="imdb|tmdb|musicbrainzartist|musicbrainzalbum">`.
//! The poster shown is `poster.jpg` or `poster.png` next to the item (folder
//! level for shows/music), since NFO files normally do not hold images.

use std::path::{Path, PathBuf};

use channelflow_plugin_api::media::CatalogItem;

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "mov", "m2ts", "ts", "webm", "wmv", "m4v", "mpg", "mpeg"];
const AUDIO_EXTS: &[&str] = &["mp3", "flac", "m4a", "aac", "ogg", "oga", "wma", "opus", "wav"];

/// What one NFO file told us.
#[derive(Debug, Default, Clone)]
pub struct NfoInfo {
    pub title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub imdb: Option<String>,
    pub tmdb: Option<i64>,
    pub musicbrainz_artist: Option<String>,
    pub musicbrainz_album: Option<String>,
}

pub fn parse_nfo(path: &Path) -> Option<NfoInfo> {
    let xml = std::fs::read_to_string(path).ok()?;
    let mut info = NfoInfo {
        title: tag(&xml, "title").map(|value| clean(&value)),
        year: tag(&xml, "year")
            .and_then(|v| clean(&v).parse::<i32>().ok())
            .or_else(|| tag(&xml, "releasedate").and_then(|v| clean(&v).get(..4).and_then(|s| s.parse().ok()))),
        overview: tag(&xml, "plot")
            .or_else(|| tag(&xml, "outline"))
            .map(|value| clean(&value))
            .filter(|v| !v.is_empty()),
        imdb: tag(&xml, "uniqueid").filter(|v| v.contains("imdb")).and_then(|v| {
            tag(&xml, "uniqueid")
                .and_then(|_| snippet(&xml, "uniqueid", "imdb"))
                .map(|value| clean(&value))
        }),
        tmdb: tag(&xml, "tmdbid")
            .or_else(|| tag(&xml, "tmdb"))
            .and_then(|v| clean(&v).parse::<i64>().ok()),
        musicbrainz_artist: tag(&xml, "musicbrainzartistid")
            .or_else(|| snippet(&xml, "uniqueid", "musicbrainzartist"))
            .map(|value| clean(&value)),
        musicbrainz_album: tag(&xml, "musicbrainzalbumid")
            .or_else(|| snippet(&xml, "uniqueid", "musicbrainzalbum"))
            .map(|value| clean(&value)),
    };
    if info.title.as_deref().unwrap_or("").is_empty() {
        return None;
    }
    Some(info)
}

fn clean(value: &str) -> String {
    value.trim().to_string()
}

/// `<name>…</name>`, whatever attributes the tag carries.
fn tag(xml: &str, name: &str) -> Option<String> {
    let pattern = format!(r"<{name}(?:\s[^>]*)?>(.*?)</{name}>", name = regex::escape(name));
    let re = regex::RegexBuilder::new(&pattern)
        .dot_matches_new_line(true)
        .case_insensitive(true)
        .build()
        .ok()?;
    re.captures(xml)?.get(1).map(|m| m.as_str().to_string())
}

/// The value of a `<uniqueid type="…">` whose type contains `wanted`.
fn snippet(xml: &str, name: &str, wanted: &str) -> Option<String> {
    let pattern = format!(r#"<{name}[^>]*type=["']?[^"'>]*{wanted}[^"'>]*["']?[^>]*>(.*?)</{name}>"#);
    let re = regex::RegexBuilder::new(&pattern)
        .dot_matches_new_line(true)
        .case_insensitive(true)
        .build()
        .ok()?;
    re.captures(xml)?.get(1).map(|m| m.as_str().to_string())
}

fn is_media_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            VIDEO_EXTS.contains(&ext.as_str()) || AUDIO_EXTS.contains(&ext.as_str())
        })
        .unwrap_or(false)
}

fn poster_in(dir: &Path) -> Option<PathBuf> {
    for name in ["poster.jpg", "poster.png", "poster.jpeg"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn is_media_kind_file(kind: &str, path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            match kind {
                "music" | "musicvideos" => AUDIO_EXTS.contains(&ext.as_str()) || VIDEO_EXTS.contains(&ext.as_str()),
                _ => VIDEO_EXTS.contains(&ext.as_str()),
            }
        })
        .unwrap_or(false)
}

/// One row destined for the base catalog.
pub struct ScannedItem {
    pub kind: &'static str,
    pub catalog: CatalogItem,
    pub poster: Option<PathBuf>,
}

fn provider_match(info: &NfoInfo) -> Option<String> {
    info.imdb
        .as_deref()
        .map(|value| format!("imdb:{value}"))
        .or_else(|| info.tmdb.map(|value| format!("tmdb:{value}")))
}

fn unique_id(info: &NfoInfo, fallback: &str) -> String {
    provider_match(info)
        .or_else(|| info.musicbrainz_artist.clone())
        .or_else(|| info.musicbrainz_album.clone())
        .unwrap_or_else(|| fallback.to_string())
}

/// Scan `root` for catalog items of the connection's `kind`
/// (movie/series/music/musicvideo).
pub fn scan_folder(root: &str, kind: &str) -> Vec<ScannedItem> {
    let mut items = Vec::new();
    let root_path = Path::new(root);
    let mut dirs = vec![root_path.to_path_buf()];
    let mut seen_dirs = std::collections::HashSet::new();
    while let Some(dir) = dirs.pop() {
        let canonical = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !seen_dirs.insert(canonical) {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut nfo_files = Vec::new();
        let mut child_dirs = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                child_dirs.push(path);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("nfo"))
            {
                nfo_files.push(path);
            }
        }
        dirs.extend(child_dirs);

        // Series: a tvshow.nfo in the folder.
        if let Some(path) = nfo_files.iter().find(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case("tvshow"))
        }) {
            if kind.eq_ignore_ascii_case("tv") || kind.eq_ignore_ascii_case("tvshows") {
                if let Some(info) = parse_nfo(path) {
                    let fallback = dir
                        .canonicalize()
                        .ok()
                        .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_else(|| "series".to_string());
                    let remote = unique_id(&info, &fallback);
                    let poster = poster_in(&dir);
                    items.push(ScannedItem {
                        kind: "series",
                        poster,
                        catalog: CatalogItem::new("series", &remote, info.title.as_deref().unwrap_or(&fallback))
                            .year(info.year)
                            .overview(info.overview.clone())
                            .match_id(provider_match(&info))
                            .library(kind),
                    });
                }
            }
            continue;
        }

        // Artists and albums: artist.nfo / album.nfo in the folder.
        if kind.eq_ignore_ascii_case("music") {
            if let Some(path) = nfo_files.iter().find(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("artist"))
            }) {
                if let Some(info) = parse_nfo(path) {
                    let fallback = dir
                        .canonicalize()
                        .ok()
                        .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_else(|| "artist".to_string());
                    let remote = info.musicbrainz_artist.clone().unwrap_or_else(|| fallback.clone());
                    items.push(ScannedItem {
                        kind: "artist",
                        poster: poster_in(&dir),
                        catalog: CatalogItem::new("artist", &remote, info.title.as_deref().unwrap_or(&fallback))
                            .match_id(info.musicbrainz_artist.clone()),
                    });
                }
            }
            if let Some(path) = nfo_files.iter().find(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("album"))
            }) {
                if let Some(info) = parse_nfo(path) {
                    let fallback = dir
                        .canonicalize()
                        .ok()
                        .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_else(|| "album".to_string());
                    let remote = info.musicbrainz_album.clone().unwrap_or_else(|| fallback.clone());
                    items.push(ScannedItem {
                        kind: "album",
                        poster: poster_in(&dir),
                        catalog: CatalogItem::new("album", &remote, info.title.as_deref().unwrap_or(&fallback))
                            .year(info.year)
                            .overview(info.overview.clone())
                            .match_id(info.musicbrainz_album.clone()),
                    });
                }
            }
        }

        // Movie / music video files: a sibling <name>.nfo beside the media file.
        if kind.eq_ignore_ascii_case("movies") || kind.eq_ignore_ascii_case("musicvideos") {
            for nfo in &nfo_files {
                let stem = nfo.file_stem().and_then(|stem| stem.to_str()).unwrap_or("");
                if stem.eq_ignore_ascii_case("tvshow")
                    || stem.eq_ignore_ascii_case("artist")
                    || stem.eq_ignore_ascii_case("album")
                {
                    continue;
                }
                let media = dir.join(format!("{stem}.*"));
                let has_media = std::fs::read_dir(&dir).map(|read| {
                    read.flatten().any(|entry| {
                        entry.path().is_file()
                            && entry
                                .path()
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .is_some_and(|s| s == stem)
                            && is_media_kind_file(kind, &entry.path())
                    })
                }).unwrap_or(false);
                if !has_media {
                    continue;
                }
                if let Some(info) = parse_nfo(nfo) {
                    let fallback = stem.to_string();
                    let remote = unique_id(&info, &fallback);
                    let item_kind = if kind.eq_ignore_ascii_case("musicvideos") {
                        "musicvideo"
                    } else {
                        "movie"
                    };
                    items.push(ScannedItem {
                        kind: item_kind,
                        poster: poster_in(&dir)
                            .or_else(|| nfo.with_extension("jpg").is_file().then(|| nfo.with_extension("jpg")))
                            .or_else(|| nfo.with_extension("png").is_file().then(|| nfo.with_extension("png"))),
                        catalog: CatalogItem::new(item_kind, &remote, info.title.as_deref().unwrap_or(&fallback))
                            .year(info.year)
                            .overview(info.overview.clone())
                            .match_id(provider_match(&info))
                            .library(kind),
                    });
                }
            }
        }
    }
    items
}