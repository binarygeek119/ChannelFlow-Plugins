//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.news`, so the News page's calls to
//! `GET /`, `PUT /`, and `POST /preview` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use crate::settings::{NewsSettings, SETTINGS_KEY};
use crate::NewsState;

pub fn router(state: Arc<NewsState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/preview", get(preview_headlines))
        .with_state(state)
}

/// The TTS voice options the form renders from.
const TTS_VOICES: [&str; 10] = [
    "en-US", "en-GB", "es", "fr", "de", "it", "pt", "ja", "ko", "zh-CN",
];

fn view(settings: &NewsSettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "tts_engines": ["google", "ai"],
            "tts_voices": TTS_VOICES,
        }
    })
}

async fn get_settings(State(state): State<Arc<NewsState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<NewsState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let settings = match NewsSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("news settings updated");
    Json(view(&settings)).into_response()
}

/// Fetch every enabled feed and pull the headlines, so the page can show what
/// the newscast would lead with.
async fn preview_headlines(State(state): State<Arc<NewsState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let limit = settings.article_count as usize;
    let mut feeds = Vec::new();
    for feed in settings.feeds.iter().filter(|feed| feed.enabled) {
        let mut entry = json!({ "url": feed.url });
        match state.http.get(&feed.url).send().await {
            Ok(response) if response.status().is_success() => {
                match response.text().await {
                    Ok(body) => {
                        let headlines = extract_headlines(&body, limit);
                        entry["headlines"] = Value::Array(
                            headlines.into_iter().map(Value::String).collect(),
                        );
                    }
                    Err(error) => {
                        entry["error"] = Value::String(format!("unreadable feed: {error}"));
                    }
                }
            }
            Ok(response) => {
                entry["error"] =
                    Value::String(format!("feed answered {}", response.status()));
            }
            Err(_) => {
                entry["error"] = Value::String("could not reach the feed".to_string());
            }
        }
        feeds.push(entry);
    }
    Json(json!({ "feeds": feeds })).into_response()
}

/// Pull the first `limit` `<title>` inside `<item>` blocks, without pulling in
/// a full XML parser. Basic entity unescaping, nothing more.
fn extract_headlines(body: &str, limit: usize) -> Vec<String> {
    let mut headlines = Vec::new();
    // The first fragment holds everything before the first <item> (channel
    // metadata, including its own <title>); items start at index 1.
    for (index, item) in body.split("<item").enumerate() {
        if index == 0 {
            continue;
        }
        let Some(start) = item.find("<title>") else { continue };
        let Some(end) = item[start..].find("</title>") else { continue };
        let raw = &item[start + "<title>".len()..start + end];
        let trimmed = raw.trim();
        let unwrapped = if trimmed.starts_with("<![CDATA[") && trimmed.ends_with("]]>") {
            &trimmed["<![CDATA[".len()..trimmed.len() - "]]>".len()]
        } else {
            trimmed
        };
        let title = unescape(unwrapped).trim().to_string();
        if !title.is_empty() {
            headlines.push(title);
        }
        if headlines.len() >= limit {
            break;
        }
    }
    headlines
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_item_titles_only() {
        let body = "<rss><channel><title>Channel</title><item><title>First story</title></item>\
            <item><title><![CDATA[Second &amp; longer story]]></title></item><item><title>Third</title></item></channel></rss>";
        let headlines = extract_headlines(body, 2);
        assert_eq!(headlines, ["First story", "Second & longer story"]);
    }

    #[test]
    fn unescapes_entities() {
        assert_eq!(unescape("AT&amp;T &quot;Q&quot; &lt;HTML&gt;"), "AT&T \"Q\" <HTML>");
    }
}