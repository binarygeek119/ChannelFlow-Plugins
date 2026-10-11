//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.offair`, so the Off Air page's calls to
//! `GET /` and `PUT /` resolve there.
//!
//! This is the v1.0.0 EBS behavior: settings plus custom off-air slate uploads
//! (per region) and a preview of the effective slate — a custom upload, or the
//! bundled stock slate for the chosen aspect. The slate the encoder would use
//! is resolved the same way: custom slate first, then the stock 16:9/4:3
//! image, exactly as v1.0.0's EbsService did.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::settings::{OffAirSettings, SETTINGS_KEY};
use crate::OffAirState;

/// The stock slates this plugin ships, per variant + aspect. The page asks for
/// the effective one; the route serves a custom upload or redirects here.
const STOCK_SLATES: &[(&str, &str, &str)] = &[
    // (variant, 16:9 file, 4:3 file)
    ("usa", "offline_usa_16_9.jpg", "offline_usa_4_3.jpg"),
    ("international", "offline_world_16_9.jpg", "offline_world_4_3.jpg"),
];

pub fn router(state: Arc<OffAirState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/slates/{variant}", post(upload_slate).delete(delete_slate))
        .route("/slates/{variant}/image", get(slate_image))
        .with_state(state)
}

/// The settings plus the option lists the form renders from, the stock slates
/// available, and which variants have a custom upload.
fn view(state: &OffAirState, settings: &OffAirSettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "display_modes": ["slate_image", "color_bars", "static"],
            "slate_variants": ["usa", "international"],
            "audio_modes": ["background_music", "white_noise", "silence", "beep_tone"],
            "music_sources": ["all_music_libraries", "named_library", "local_packs"],
        },
        "stock_slates": stock_slate_view(),
        "custom_slates": custom_slate_status(state),
    })
}

fn stock_slate_view() -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (variant, wide, square) in STOCK_SLATES {
        map.insert(
            (*variant).to_string(),
            json!({
                "wide": format!("/plugin/com.channelflow.offair/web/media/{wide}"),
                "square": format!("/plugin/com.channelflow.offair/web/media/{square}"),
            }),
        );
    }
    serde_json::Value::Object(map)
}

/// Which variants have a custom slate uploaded, as `variant -> file name or
/// null`.
fn custom_slate_status(state: &OffAirState) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (variant, _, _) in STOCK_SLATES {
        let value = custom_slate_file(&state.dir, variant)
            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null);
        map.insert((*variant).to_string(), value);
    }
    serde_json::Value::Object(map)
}

/// The uploaded slate file for a variant, if one exists.
fn custom_slate_file(dir: &Path, variant: &str) -> Option<PathBuf> {
    for ext in ["png", "jpg"] {
        let candidate = dir.join("slates").join(format!("{variant}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn valid_variant(variant: &str) -> bool {
    STOCK_SLATES.iter().any(|(v, _, _)| *v == variant)
}

async fn get_settings(State(state): State<Arc<OffAirState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(view(&state, &settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<OffAirState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let settings = match OffAirSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("off-air settings updated");
    Json(view(&state, &settings)).into_response()
}

/// Upload a custom slate for a variant — the raw PNG/JPG bytes are the body,
/// exactly like v1.0.0's `POST api/ebs/slates/{variant}`.
async fn upload_slate(
    State(state): State<Arc<OffAirState>>,
    AxumPath(variant): AxumPath<String>,
    bytes: Bytes,
) -> Response {
    if !valid_variant(&variant) {
        return fail(
            StatusCode::BAD_REQUEST,
            "variant must be usa or international".to_string(),
        );
    }
    if bytes.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "an image file is required".to_string());
    }
    if bytes.len() > 2 * 1024 * 1024 {
        return fail(StatusCode::BAD_REQUEST, "the slate image is larger than 2 MB".to_string());
    }
    let ext = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else {
        return fail(
            StatusCode::BAD_REQUEST,
            "the slate must be a PNG or JPEG image".to_string(),
        );
    };
    let dir = state.dir.join("slates");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not create the slates store: {error}"),
        );
    }
    // Remove any older-format custom slate for this variant, then write.
    for old in ["png", "jpg"] {
        if old != ext {
            let _ = std::fs::remove_file(dir.join(format!("{variant}.{old}")));
        }
    }
    if let Err(error) = std::fs::write(dir.join(format!("{variant}.{ext}")), &bytes) {
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not store the slate: {error}"),
        );
    }
    Json(json!({
        "variant": variant,
        "custom_slates": custom_slate_status(&state),
    }))
    .into_response()
}

/// Delete a variant's custom slate (v1.0.0's `DELETE api/ebs/slates/{variant}`).
async fn delete_slate(
    State(state): State<Arc<OffAirState>>,
    AxumPath(variant): AxumPath<String>,
) -> Response {
    if !valid_variant(&variant) {
        return fail(
            StatusCode::BAD_REQUEST,
            "variant must be usa or international".to_string(),
        );
    }
    for old in ["png", "jpg"] {
        let _ = std::fs::remove_file(state.dir.join("slates").join(format!("{variant}.{old}")));
    }
    Json(json!({ "custom_slates": custom_slate_status(&state) })).into_response()
}

#[derive(Deserialize)]
struct SlateImageQuery {
    /// `0` = 16:9, `1` = 4:3.
    #[serde(default)]
    aspect: Option<u8>,
}

/// The effective slate for a variant + aspect: a custom upload if one exists,
/// else the bundled stock image — how Off Air actually plays, for the page's
/// preview.
async fn slate_image(
    State(state): State<Arc<OffAirState>>,
    AxumPath(variant): AxumPath<String>,
    Query(query): Query<SlateImageQuery>,
) -> Response {
    let Some((v, wide, square)) = STOCK_SLATES.iter().find(|(v, _, _)| *v == variant) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "variant must be usa or international".to_string(),
        );
    };
    if let Some(custom) = custom_slate_file(&state.dir, v) {
        let mime = if custom.extension().and_then(|e| e.to_str()) == Some("png") {
            "image/png"
        } else {
            "image/jpeg"
        };
        match std::fs::read(&custom) {
            Ok(bytes) => {
                return (
                    [(header::CONTENT_TYPE, mime)],
                    axum::body::Bytes::from(bytes),
                )
                    .into_response();
            }
            Err(error) => {
                state.logger.warn(&format!("could not read the custom slate: {error}"));
            }
        }
    }
    let file = if query.aspect == Some(1) { square } else { wide };
    Redirect::to(&format!("/plugin/com.channelflow.offair/web/media/{file}")).into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}