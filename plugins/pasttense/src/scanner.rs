//! Scanning for the Past Tense News source.
//!
//! The selected folder holds **events** — one folder per news event
//! ("1992 LA Riots News coverage", "Apollo 11 News coverage") — and inside
//! that folder sit the event's coverage videos ("1992 Los Angeles riots news
//! coverage.mkv", "NBC News Coverage of Apollo 11 Part 11.mkv"). Each event
//! folder becomes one catalog item, shown like a TV show but without seasons.
//! A `poster.jpg`/`poster.png` in the event folder is its artwork.

use std::path::{Path, PathBuf};

use channelflow_plugin_api::media::CatalogItem;

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "mov", "m2ts", "ts", "webm", "wmv", "m4v", "mpg", "mpeg"];

/// One event destined for the base catalog.
pub struct ScannedEvent {
    pub catalog: CatalogItem,
    pub poster: Option<PathBuf>,
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
    dir.file_name()
        .map(|name| {
            name.to_string_lossy()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .take(64)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "event".to_string())
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