//! Local music packs from 1.0.0.
//!
//! A pack is a folder of audio files under `<config>/music/<id>` glad to
//! downloaded from the catalog's Google Drive link. Off-air background music
//! with `music_source = local_packs` picks a random track from the pack the
//! current date activates — Anytime, Fall (Sep–Nov) or Winter (Dec–Feb),
//! exactly like 1.0.0's `MusicPackService`.

use std::path::{Path, PathBuf};

use chrono::Datelike;
use serde::{Deserialize, Serialize};

/// The shipped pack catalog (id, display name, season, drive link).
const CATALOG_JSON: &str = include_str!("../music-packs.json");

/// Audio files a pack can hold.
const AUDIO_EXTENSIONS: [&str; 7] = [".mp3", ".m4a", ".aac", ".ogg", ".flac", ".wav", ".wma"];

/// One row of the catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub season: String,
    #[serde(default)]
    pub version: String,
    pub google_drive_file_id: Option<String>,
}

/// The state the page shows for one pack.
#[derive(Debug, Serialize)]
pub struct PackStatus {
    pub id: String,
    pub name: String,
    pub season: String,
    pub track_count: usize,
    pub installed: bool,
    pub active: bool,
    pub plays_when: String,
}

/// Parse the bundled catalog.
pub fn catalog() -> Vec<Pack> {
    serde_json::from_str::<serde_json::Value>(CATALOG_JSON)
        .ok()
        .and_then(|value| value.get("packs").cloned())
        .and_then(|value| serde_json::from_value::<Vec<Pack>>(value).ok())
        .unwrap_or_default()
}

/// `<config>/music` — where packs live.
pub fn music_root(config_root: &Path) -> PathBuf {
    config_root.join("music")
}

fn pack_folder(root: &Path, id: &str) -> PathBuf {
    music_root(root).join(id)
}

/// Every audio file in a pack, in a stable order.
fn list_tracks(root: &Path, id: &str) -> Vec<PathBuf> {
    let folder = pack_folder(root, id);
    walk_audio(&folder)
}

fn walk_audio(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_audio(&path));
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let lower = ext.to_ascii_lowercase();
            if AUDIO_EXTENSIONS.iter().any(|want| want == &format!(".{lower}")) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn season_of(date: chrono::NaiveDate) -> &'static str {
    match date.month() {
        9 | 10 | 11 => "fall",
        12 | 1 | 2 => "winter",
        _ => "anytime",
    }
}

/// The pack the current date activates, out of the installed ones: the
/// season's pack first, then Anytime.
pub fn active_pack_id(root: &Path, date: chrono::NaiveDate) -> Option<String> {
    let season = season_of(date);
    let catalog = catalog();
    for pack in catalog.iter() {
        if pack.season == season && !list_tracks(root, &pack.id).is_empty() {
            return Some(pack.id.clone());
        }
    }
    catalog
        .iter()
        .find(|pack| pack.season == "anytime" && !list_tracks(root, &pack.id).is_empty())
        .map(|pack| pack.id.clone())
}

/// A random track from the active pack, if any.
pub fn pick_track(root: &Path, date: chrono::NaiveDate) -> Option<PathBuf> {
    let id = active_pack_id(root, date)?;
    let tracks = list_tracks(root, &id);
    if tracks.is_empty() {
        return None;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    Some(tracks[(nanos as usize) % tracks.len()].clone())
}

/// The per-pack status rows the page shows.
pub fn list_status(root: &Path, date: chrono::NaiveDate) -> Vec<PackStatus> {
    let active = active_pack_id(root, date);
    catalog()
        .into_iter()
        .map(|pack| {
            let tracks = list_tracks(root, &pack.id);
            let id = pack.id.clone();
            PackStatus {
                plays_when: plays_when(&pack.season),
                id,
                name: pack.name,
                season: pack.season,
                track_count: tracks.len(),
                installed: !tracks.is_empty(),
                active: active.as_deref() == Some(pack.id.as_str()),
            }
        })
        .collect()
}

fn plays_when(season: &str) -> String {
    match season {
        "anytime" => "Plays whenever no other pack matches the date.".to_string(),
        "fall" => "Plays 1 Sep — 30 Nov after it is downloaded.".to_string(),
        "winter" => "Plays 1 Dec — end of Feb after it is downloaded.".to_string(),
        other => format!("Plays while the {other} pack is active."),
    }
}

/// Download a pack from Google Drive and extract its audio into its folder.
pub async fn download(root: &Path, http: &reqwest::Client, pack: &Pack) -> Result<(), String> {
    let file_id = pack
        .google_drive_file_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{} has no Google Drive file yet.", pack.name))?;
    let music = music_root(root);
    std::fs::create_dir_all(&music).map_err(|error| error.to_string())?;

    let zip_path = music.join(format!("{}.zip", pack.id));
    let staging = music.join(format!("{}.staging", pack.id));
    let dest = pack_folder(root, &pack.id);

    let result = download_drive_zip(http, file_id, &zip_path).await;
    if let Err(error) = result {
        let _ = std::fs::remove_file(&zip_path);
        return Err(error);
    }
    if !is_zip(&zip_path) {
        let _ = std::fs::remove_file(&zip_path);
        return Err("the download is not a zip — check the Google Drive share link.".to_string());
    }

    let extracted = extract_zip(&zip_path, &staging);
    let _ = std::fs::remove_file(&zip_path);
    if let Err(error) = extracted {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    flatten_and_clean(&staging);
    if walk_audio(&staging).is_empty() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("{} zip did not contain any audio files.", pack.name));
    }

    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    std::fs::rename(&staging, &dest).map_err(|error| error.to_string())?;
    let count = walk_audio(&dest).len();
    tracing::info!("installed music pack {} ({count} tracks)", pack.id);
    Ok(())
}

/// Remove a pack's folder.
pub fn delete(root: &Path, id: &str) {
    let folder = pack_folder(root, id);
    if folder.exists() {
        let _ = std::fs::remove_dir_all(&folder);
        tracing::info!("removed music pack {id}");
    }
}

async fn download_drive_zip(
    http: &reqwest::Client,
    file_id: &str,
    destination: &Path,
) -> Result<(), String> {
    let first_url = format!("https://drive.google.com/uc?export=download&id={}", url_encode(file_id));
    let first = http
        .get(&first_url)
        .send()
        .await
        .map_err(|_| "could not reach Google Drive".to_string())?;
    if !first.status().is_success() {
        return Err(format!("Google Drive answered {}", first.status()));
    }
    let is_html = first
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("html"))
        .unwrap_or(false);

    let bytes = if is_html {
        // The virus-scan confirmation page: rebuild the download URL.
        let html = first.text().await.map_err(|_| "unreadable Drive page".to_string())?;
        let url = build_drive_download_url(&html, file_id)?;
        let retry = http
            .get(&url)
            .send()
            .await
            .map_err(|_| "could not reach Google Drive".to_string())?;
        if !retry.status().is_success() {
            return Err(format!("Google Drive answered {}", retry.status()));
        }
        retry.bytes().await.map_err(|_| "could not read the pack".to_string())?
    } else {
        first.bytes().await.map_err(|_| "could not read the pack".to_string())?
    };

    std::fs::write(destination, &bytes).map_err(|error| error.to_string())?;
    Ok(())
}

/// The confirm URL from Google Drive's virus-scan page.
fn build_drive_download_url(html: &str, file_id: &str) -> Result<String, String> {
    let mut query = vec![("id".to_string(), file_id.to_string()), ("export".to_string(), "download".to_string())];
    for name in ["confirm", "uuid", "at"] {
        if let Some(value) = find_input_value(html, name) {
            query.push((name.to_string(), value));
        }
    }
    if !query.iter().any(|(key, _)| key == "uuid") {
        return Err("Google Drive returned a web page instead of the zip.".to_string());
    }
    if !query.iter().any(|(key, _)| key == "confirm") {
        query.push(("confirm".to_string(), "t".to_string()));
    }
    Ok(format!(
        "https://drive.usercontent.google.com/download?{}",
        query
            .iter()
            .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    ))
}

fn find_input_value(html: &str, name: &str) -> Option<String> {
    let marker = format!("name=\"{name}\"");
    let start = html.find(&marker)?;
    let rest = &html[start + marker.len()..];
    let value = rest.find("value=\"")?;
    let after = &rest[value + "value=\"".len()..];
    Some(after.split('"').next()?.to_string())
}

fn is_zip(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .map(|bytes| bytes.starts_with(b"PK"))
        .unwrap_or(false)
}

fn extract_zip(zip_path: &Path, staging: &Path) -> Result<(), String> {
    if staging.exists() {
        let _ = std::fs::remove_dir_all(staging);
    }
    std::fs::create_dir_all(staging).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(zip_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    archive.extract(staging).map_err(|error| error.to_string())?;
    Ok(())
}

/// A zip that unzips to exactly one inner folder gets flattened, so the pack
/// folder holds the audio directly; junk files are dropped.
fn flatten_and_clean(staging: &Path) {
    let entries: Vec<PathBuf> = std::fs::read_dir(staging)
        .map(|read| {
            read.flatten()
                .map(|entry| entry.path())
                .filter(|path| !is_junk(path))
                .collect()
        })
        .unwrap_or_default();
    if entries.len() == 1 && entries[0].is_dir() {
        let inner = entries[0].clone();
        if let Ok(children) = std::fs::read_dir(&inner) {
            for child in children.flatten() {
                let path = child.path();
                let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                let dest = staging.join(&name);
                if !dest.exists() {
                    let _ = std::fs::rename(&path, &dest);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&inner);
    }
    if let Ok(dirs) = std::fs::read_dir(staging) {
        for entry in dirs.flatten() {
            let path = entry.path();
            if is_junk(&path) {
                let _ = std::fs::remove_dir_all(&path);
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

fn is_junk(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    name == "__MACOSX" || name == ".DS_Store" || name == ".DS_Store" || name == "Thumbs.db"
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_the_three_classic_packs() {
        let packs = catalog();
        let ids: Vec<&str> = packs.iter().map(|pack| pack.id.as_str()).collect();
        assert_eq!(ids, ["anytime", "fall", "winter"]);
        assert!(packs.iter().all(|pack| pack.google_drive_file_id.is_some()));
    }

    #[test]
    fn seasons_resolve_to_the_right_pack() {
        let fall = chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        assert_eq!(season_of(fall), "fall");
        let winter = chrono::NaiveDate::from_ymd_opt(2027, 1, 5).unwrap();
        assert_eq!(season_of(winter), "winter");
        let summer = chrono::NaiveDate::from_ymd_opt(2026, 7, 4).unwrap();
        assert_eq!(season_of(summer), "anytime");
    }

    #[test]
    fn active_pack_prefers_the_season_then_anytime() {
        let root = std::env::temp_dir().join("cf-pack-test");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(root.join("music").join("fall"));
        let _ = std::fs::create_dir_all(root.join("music").join("anytime"));
        std::fs::write(root.join("music").join("fall").join("t1.mp3"), b"x").unwrap();
        std::fs::write(root.join("music").join("anytime").join("t2.mp3"), b"y").unwrap();

        let fall_date = chrono::NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        assert_eq!(active_pack_id(&root, fall_date).as_deref(), Some("fall"));
        let summer = chrono::NaiveDate::from_ymd_opt(2026, 7, 4).unwrap();
        assert_eq!(active_pack_id(&root, summer).as_deref(), Some("anytime"));

        assert!(pick_track(&root, summer).is_some(), "anytime has a track");
        let _ = std::fs::remove_dir_all(&root);
    }
}