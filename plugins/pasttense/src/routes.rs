//! The Past Tense News plugin's API: a server-side folder browser so the page
//! can walk to the events folder and pick it. Mounted under
//! `/api/plugins/com.channelflow.pasttense`.

use std::path::{Path, PathBuf};

use axum::{
    extract::Query,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router {
    Router::new().route("/browse", get(browse))
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