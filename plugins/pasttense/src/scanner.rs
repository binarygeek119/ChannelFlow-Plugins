//! Scanning for the Past Tense News source.
//!
//! The selected folder holds **events** — one folder per news event
//! ("1992 LA Riots News coverage", "Apollo 11 News coverage") — and inside
//! that folder sit the event's coverage videos ("1992 Los Angeles riots news
//! coverage.mkv", "NBC News Coverage of Apollo 11 Part 11.mkv"). Each event
//! folder becomes one catalog item (kind `news`), shown like a TV show; its
//! coverage videos are listed like episodes, with artwork from the per-video
//! `*.nfo` (title/plot/year/youtubeid) and the youtube-dl `*.info.json`
//! (a real thumbnail and watch URL) that sit next to each file.

use std::path::{Path, PathBuf};

use channelflow_plugin_api::media::CatalogItem;

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "mov", "m2ts", "ts", "webm", "wmv", "m4v", "mpg", "mpeg"];

/// One event destined for the base catalog.
pub struct ScannedEvent {
    pub catalog: CatalogItem,
    pub poster: Option<PathBuf>,
}

/// One coverage video inside an event folder, as the event page lists it.
#[derive(Debug, Clone)]
pub struct NewsFile {
    pub title: String,
    pub year: Option<i32>,
    pub plot: Option<String>,
    /// A watch URL (the youtube-dl `webpage_url`, or one built from the nfo's
    /// youtube id) — this is what "Play" opens.
    pub web_url: Option<String>,
    /// Artwork for the file: the youtube-dl thumbnail, or a YouTube thumbnail
    /// built from the nfo's youtube id.
    pub thumb_url: Option<String>,
    pub file: PathBuf,
}

fn poster_in(dir: &Path) -> Option<PathBuf> {
    for name in ["poster.jpg", "poster.png", "poster.jpeg", "thumb.jpg", "thumb.png"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn count_videos(dir: &Path) -> usize {
    let mut count = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        if let Ok(read) = std::fs::read_dir(&path) {
            for entry in read.flatten() {
                let entry = entry.path();
                if entry.is_dir() {
                    stack.push(entry);
                } else if entry
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        let ext = ext.to_ascii_lowercase();
                        VIDEO_EXTS.contains(&ext.as_str())
                    })
                {
                    count += 1;
                }
            }
        }
    }
    count
}

/// A display title for the folder: underscores to spaces, collapsed.
pub fn clean_title(name: &str) -> String {
    let replaced = name.replace(['_', '.'], " ");
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The leading 4-digit year in a folder name, if one is there.
pub fn year_from(name: &str) -> Option<i32> {
    name.split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == 4)
        .and_then(|part| part.parse().ok())
        .filter(|year: &i32| (1900..=2099).contains(year))
}

fn remote_id(dir: &Path) -> String {
    // The absolute folder path: the Media page passes it back to this plugin's
    // /files endpoint to list the event's coverage videos.
    dir.to_string_lossy().into_owned()
}

/// The text inside a simple `<name>…</name>` tag (no attributes) in an nfo.
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let start = xml.find(&open)?;
    let after = &xml[start + open.len()..];
    let gt = after.find('>')?;
    let content = &after[gt + 1..];
    let end = content.find(&close)?;
    Some(content[..end].trim().to_string())
}

/// The event's coverage videos, as the event page lists them (episode-style).
pub fn scan_event_files(dir: &Path) -> Vec<NewsFile> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut videos: Vec<(PathBuf, String)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            let stem = stem.to_string();
            videos.push((path, stem));
        }
    }
    videos.sort_by(|left, right| left.1.cmp(&right.1));
    videos
        .into_iter()
        .filter_map(|(path, stem)| {
            let mut title = clean_title(&stem);
            let mut year = None;
            let mut plot = None;
            let mut youtube_id = None;
            if let Ok(nfo) = std::fs::read_to_string(dir.join(format!("{stem}.nfo"))) {
                if let Some(value) = tag(&nfo, "title") {
                    if !value.is_empty() {
                        title = value;
                    }
                }
                plot = tag(&nfo, "plot").filter(|value| !value.is_empty());
                year = tag(&nfo, "year").and_then(|value| value.parse::<i32>().ok());
                youtube_id = tag(&nfo, "youtubeid").filter(|value| !value.is_empty());
            }
            let mut thumb_url = None;
            let mut web_url = None;
            if let Ok(source) = std::fs::read_to_string(dir.join(format!("{stem}.info.json"))) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&source) {
                    thumb_url = meta
                        .get("thumbnail")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    web_url = meta
                        .get("webpage_url")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    if web_url.is_none() {
                        web_url = meta
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(|id| format!("https://www.youtube.com/watch?v={id}"));
                    }
                }
            }
            if thumb_url.is_none() {
                if let Some(id) = youtube_id.as_deref() {
                    thumb_url = Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"));
                }
            }
            Some(NewsFile {
                title,
                year,
                plot,
                web_url,
                thumb_url,
                file: path,
            })
        })
        .collect()
}

/// Scan the news folder: each subfolder is one event.
pub fn scan_folder(root: &str) -> Vec<ScannedEvent> {
    let mut events = Vec::new();
    let Ok(read) = std::fs::read_dir(root) else {
        return events;
    };
    let mut dirs: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
        })
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let title = clean_title(&name);
        let year = year_from(&name);
        let videos = count_videos(&dir);
        let overview = if videos > 0 {
            Some(format!("{videos} coverage video(s)"))
        } else {
            None
        };
        events.push(ScannedEvent {
            poster: poster_in(&dir),
            catalog: CatalogItem::new("news", &remote_id(&dir), &title)
                .year(year)
                .overview(overview.clone())
                .library("news"),
        });
    }
    events
}