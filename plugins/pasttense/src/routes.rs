//! The Past Tense News plugin's API endpoints. Mounted under
//! `/api/plugins/com.channelflow.pasttense`:
//!   `/browse` — a server-side folder browser so the page can walk to the
//!              events folder and pick it;
//!   `/files`  — the coverage videos inside one event folder, as the Media
//!              page's episode-style list needs them.

use std::path::{Path, PathBuf};

use axum::{
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::scanner::scan_event_files;

pub fn router() -> Router {
    Router::new()
        .route("/browse", get(browse))
        .route("/files", get(event_files))
}

#[derive(Deserialize)]
struct BrowseQuery {
    #[serde(default)]
    path: Option<String>,
}

/// List the directories under `path` (default `/`), plus the parent, so the
/// page can browse to the events folder and select it.
async fn browse(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let current = PathBuf::from(query.path.unwrap_or_else(|| "/".to_string()));
    let display = current.display().to_string();
    let mut entries = Vec::new();
    let mut error: Option<String> = None;
    match std::fs::read_dir(&current) {
        Ok(read) => {
            let mut dirs: Vec<String> = read
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.path().display().to_string())
                .collect();
            dirs.sort();
            if let Some(parent) = Path::new(&display).parent() {
                entries.push(json!({ "path": parent.display().to_string(), "name": ".." }));
            }
            for dir in dirs {
                let name = Path::new(&dir)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.clone());
                entries.push(json!({ "path": dir, "name": name }));
            }
        }
        Err(err) => error = Some(err.to_string()),
    }
    Json(json!({ "current": display, "entries": entries, "error": error }))
}

#[derive(Deserialize)]
struct FilesQuery {
    /// An event folder's absolute path (the catalog item's `remote_id`).
    #[serde(default)]
    path: Option<String>,
}

/// The coverage videos inside one event folder, episode-style: every file's
/// title, plot/year, a watch URL, and artwork (from its `.nfo`/`.info.json`).
async fn event_files(Query(query): Query<FilesQuery>) -> impl IntoResponse {
    let Some(path) = query.path.filter(|path| !path.trim().is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "an event folder path is required" })),
        )
            .into_response();
    };
    let dir = PathBuf::from(&path);
    if !std::fs::metadata(&dir).map(|meta| meta.is_dir()).unwrap_or(false) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such event folder" })),
        )
            .into_response();
    }
    let title = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let files: Vec<serde_json::Value> = scan_event_files(&dir)
        .into_iter()
        .map(|file| {
            json!({
                "title": file.title,
                "year": file.year,
                "plot": file.plot,
                "thumb_url": file.thumb_url,
                "web_url": file.web_url,
                "file": file.file.display().to_string(),
            })
        })
        .collect();
    Json(json!({ "path": path, "title": title, "files": files })).into_response()
}