//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.news`, so the News page's calls to
//! `GET /`, `PUT /`, and `GET /preview` resolve there.
//!
//! The preview fetches every enabled feed like v1.0.0's NewsHeadlineService:
//! RSS `<item>` and Atom `<entry>` blocks become stories with a title, a
//! summary, the feed name, and an image when the item carries one, cached for
//! ~2 minutes, then the endpoint hands back the whole newscast show — the
//! header, the anchor intro, the stories, and the outro — as v1.0.0's
//! NewsShowWriter would script it.

use std::sync::Arc;
use std::time::Instant;

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
        .route("/preview", get(preview_newscast))
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

/// The show name a header means, exactly like v1.0.0's NewsShowWriter: an
/// empty or legacy header falls back to the FlowWire brand.
pub fn resolve_show_name(header: &str) -> String {
    let value = header.trim();
    if value.is_empty() {
        return "FlowWire News".to_string();
    }
    let compact: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if compact.to_ascii_lowercase().contains("fintv")
        || compact.to_ascii_lowercase().contains("channelflow")
        || value.eq_ignore_ascii_case("FlowWire")
    {
        return "FlowWire News".to_string();
    }
    value.to_string()
}

fn default_intro(show_name: &str) -> String {
    format!("I'm Catherine Wolfe. You're watching {show_name}.")
}

fn default_outro(show_name: &str) -> String {
    format!("I'm Catherine Wolfe. Stay with {show_name}.")
}

/// Fetch every enabled feed and build the newscast show — the stories plus
/// the anchor's intro and outro — cached ~2 minutes like v1.0.0.
async fn preview_newscast(State(state): State<Arc<NewsState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let show_name = resolve_show_name(&settings.header);
    let intro = if settings.anchor_intro.trim().is_empty() {
        default_intro(&show_name)
    } else {
        settings.anchor_intro.trim().to_string()
    };
    let outro = if settings.anchor_outro.trim().is_empty() {
        default_outro(&show_name)
    } else {
        settings.anchor_outro.trim().to_string()
    };

    let stories = cached_stories(&state, &settings).await;
    Json(json!({
        "header": show_name,
        "show_header": settings.show_header,
        "headlines_only": settings.headlines_only,
        "intro": intro,
        "outro": outro,
        "stories": stories,
        "count": stories.len(),
    }))
    .into_response()
}

/// The fetched stories, serving the 2-minute cache when it is warm (or a fresh
/// fetch when it is not or the feeds changed).
async fn cached_stories(state: &Arc<NewsState>, settings: &NewsSettings) -> Vec<Value> {
    let key = feeds_key(settings);
    {
        let cache = state.headline_cache.lock().await;
        if let Some((cached_key, fetched_at, stories)) = cache.as_ref() {
            if cached_key == &key && fetched_at.elapsed().as_secs() < 2 * 60 {
                return stories.clone();
            }
        }
    }
    let limit = if settings.ai_rewrite {
        (settings.article_count * 3).clamp(8, 40) as usize
    } else {
        settings.article_count.max(1) as usize
    };
    let enabled: Vec<String> = settings
        .feeds
        .iter()
        .filter(|feed| feed.enabled)
        .map(|feed| feed.url.clone())
        .collect();
    let stories = fetch_stories(&state.http, &enabled, limit).await;
    {
        let mut cache = state.headline_cache.lock().await;
        *cache = Some((key, Instant::now(), stories.clone()));
    }
    stories
}

fn feeds_key(settings: &NewsSettings) -> String {
    settings
        .feeds
        .iter()
        .filter(|feed| feed.enabled)
        .map(|feed| feed.url.clone())
        .collect::<Vec<_>>()
        .join("|")
}

/// Fetch and parse every feed into stories, stopping once `limit` stories have
/// been found (v1.0.0 fetched until the cap too).
async fn fetch_stories(http: &reqwest::Client, urls: &[String], limit: usize) -> Vec<Value> {
    let mut stories = Vec::new();
    for url in urls {
        if stories.len() >= limit {
            break;
        }
        let Ok(response) = http.get(url).send().await else { continue };
        if !response.status().is_success() {
            continue;
        }
        let Ok(body) = response.text().await else { continue };
        for story in extract_stories(&body, url) {
            stories.push(story);
            if stories.len() >= limit {
                return stories;
            }
        }
    }
    stories
}

/// Parse RSS `<item>` and Atom `<entry>` blocks from a feed body into stories
/// with a title, summary, feed name, and an image when one is present.
fn extract_stories(body: &str, feed_url: &str) -> Vec<Value> {
    let mut stories = Vec::new();
    for block in split_blocks(body) {
        let Some(title) = tag(&block, "title") else { continue };
        if title.is_empty() {
            continue;
        }
        let summary = ["description", "summary", "content", "encoded"]
            .iter()
            .find_map(|name| tag(&block, name))
            .map(|value| clean(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let image = image_from(&block, feed_url);
        stories.push(json!({
            "title": unescape(&title),
            "summary": unescape(&summary),
            "feed": feed_name(feed_url),
            "image": image,
        }));
    }
    stories
}

/// The `<item>` and `<entry>` blocks of an RSS/Atom document, split on the
/// tags (a full XML parser is not worth the dependency).
fn split_blocks(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for open in ["<item", "<entry"] {
        for (index, fragment) in body.split(open).enumerate() {
            if index == 0 {
                continue;
            }
            blocks.push(fragment.to_string());
        }
    }
    blocks
}

/// The text inside the first `<name>…</name>` in a fragment, CDATA-aware.
fn tag(fragment: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let start = fragment.find(&open)?;
    let after = &fragment[start + open.len()..];
    let gt = after.find('>')?;
    let content = &after[gt + 1..];
    let end = content.find(&close)?;
    let raw = content[..end].trim();
    let unwrapped = if raw.starts_with("<![CDATA[") && raw.ends_with("]]>") {
        &raw["<![CDATA[".len()..raw.len() - "]]>".len()]
    } else {
        raw
    };
    Some(unwrapped.trim().to_string())
}

/// A picture for the story: enclosure/media thumbnail, or the first `<img>` in
/// the summary. Relative URLs resolve against the feed.
fn image_from(fragment: &str, feed_url: &str) -> Option<String> {
    for name in ["enclosure", "thumbnail", "content", "image", "img"] {
        let open = format!("<{name}");
        let mut search = 0;
        while let Some(rel) = fragment[search..].find(&open) {
            let start = search + rel;
            let after_open = &fragment[start + open.len()..];
            let Some(gt_rel) = after_open.find('>') else {
                break;
            };
            let attrs = &after_open[..gt_rel];
            for attr in ["href", "url", "src"] {
                if let Some(value) = attr_value(attrs, attr) {
                    if looks_like_image(&value) {
                        return Some(resolve_url(&value, feed_url));
                    }
                }
            }
            search = start + open.len() + gt_rel + 1;
        }
    }
    None
}

fn attr_value(attrs: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let marker = format!("{name}={quote}");
        if let Some(start) = attrs.find(&marker) {
            let rest = &attrs[start + marker.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    let bare = format!("{name}=");
    if attrs.contains(&bare) {
        return Some(attrs.split(&bare).nth(1)?.split([' ', '>']).next()?.to_string());
    }
    None
}

fn looks_like_image(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.contains("images")
}

/// Resolve a possibly-relative image link against the feed's base URL.
fn resolve_url(url: &str, feed_url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if url.starts_with('/') {
        let scheme = feed_url.split("://").next().unwrap_or("http");
        let host = feed_url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("");
        return format!("{scheme}://{host}{url}");
    }
    let base = feed_url.rfind('/').map(|i| &feed_url[..i + 1]).unwrap_or(feed_url);
    format!("{base}{url}")
}

fn feed_name(feed_url: &str) -> String {
    feed_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && !name.contains('.'))
        .unwrap_or(feed_url)
        .to_string()
}

/// Trim HTML tags and collapse whitespace in a summary's text.
fn clean(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        if ch == '<' {
            in_tag = true;
            continue;
        }
        if ch == '>' {
            in_tag = false;
            continue;
        }
        if !in_tag {
            text.push(ch);
        }
    }
    unescape(&text).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&#13;", " ")
        .replace('\u{00a0}', " ")
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_name_falls_back_to_flowwire() {
        assert_eq!(resolve_show_name(""), "FlowWire News");
        assert_eq!(resolve_show_name("The Evening News"), "The Evening News");
        assert_eq!(resolve_show_name("ChannelFlow News"), "FlowWire News");
    }

    #[test]
    fn extracts_stories_from_rss_and_atom() {
        let body = "<rss><channel><item><title>First story</title>\
            <description>Some &amp; fine detail.</description>\
            <enclosure url=\"https://media.example/a.jpg\" type=\"image/jpeg\"/>\
            </item><entry><title>Second story</title>\
            <content type=\"html\"><![CDATA[<p>Whole &lt;b&gt;story&lt;/b&gt; here.</p>]]></content>\
            </entry></channel></rss>";
        let stories = extract_stories(body, "https://feed.example/news");
        assert_eq!(stories.len(), 2);
        assert_eq!(stories[0]["title"], "First story");
        assert_eq!(stories[0]["summary"], "Some & fine detail.");
        assert_eq!(stories[0]["image"], "https://media.example/a.jpg");
        assert_eq!(stories[1]["title"], "Second story");
        assert_eq!(stories[1]["summary"], "Whole <b>story</b> here.");
    }

    #[test]
    fn relative_images_resolve_against_the_feed() {
        let body = "<item><title>T</title><description><img src=\"/media/pic.png\"></description></item>";
        let stories = extract_stories(body, "https://feed.example/news/rss");
        assert_eq!(stories[0]["image"], "https://feed.example/media/pic.png");
    }
}