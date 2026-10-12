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
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Local, Timelike};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::settings::{NewsSettings, SETTINGS_KEY};
use crate::NewsState;

pub fn router(state: Arc<NewsState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/preview", get(preview_newscast))
        .route("/bulletin", get(bulletin_status))
        .route("/bulletin/run", post(bulletin_run))
        .with_state(state)
}

/// The TTS voice options the form renders from.
const TTS_VOICES: [&str; 10] = [
    "en-US", "en-GB", "es", "fr", "de", "it", "pt", "ja", "ko", "zh-CN",
];

fn view(settings: &NewsSettings) -> serde_json::Value {
    let show_name = resolve_show_name(&settings.header);
    json!({
        "settings": settings,
        "options": {
            "tts_voices": TTS_VOICES,
            // The default script, so the form can pre-fill the intro/outro
            // boxes with the FlowWire lines instead of leaving them blank.
            "default_intro": default_intro(&show_name),
            "default_outro": default_outro(&show_name),
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

/// The token in the anchor's intro/outro that **just the hour** of the build
/// replaces, so the anchor can say when the newscast went to air: `@@` becomes
/// `3 P M` in 12-hour mode, or `15` in 24-hour mode.
pub const HOUR_TOKEN: &str = "@@";

/// The token that the **full build time** replaces: `@@@@` becomes `3 14 P M`
/// in 12-hour mode, or `15 14` in 24-hour mode.
pub const FULL_TIME_TOKEN: &str = "@@@@";

/// The default anchor open. Uses the full-time token so the anchor says the
/// whole build time unless the user changes the script.
pub fn default_intro(show_name: &str) -> String {
    format!("I'm Catherine Wolfe, And you're watching {show_name} At {FULL_TIME_TOKEN}")
}

/// The default anchor close, with the full-time token.
pub fn default_outro(show_name: &str) -> String {
    format!("I'm Catherine Wolfe, This was {show_name} At {FULL_TIME_TOKEN}")
}

/// Just the hour, as the anchor reads it. 24-hour keeps the hour (two digits,
/// `15`); 12-hour converts it to `3 P M` — the AM/PM letters are spaced so the
/// TTS reads them as letters rather than the words "am"/"pm".
pub fn spoken_hour(hour: u32, use_24_hour: bool) -> String {
    if use_24_hour {
        return format!("{hour:02}");
    }
    let period = if hour % 24 < 12 { "A M" } else { "P M" };
    let mut twelve = hour % 12;
    if twelve == 0 {
        twelve = 12;
    }
    format!("{twelve} {period}")
}

/// The full build time, as the anchor reads it: the hour and the minute with a
/// space between them, so the TTS says "ten fourteen", not "ten-colon-fourteen".
/// 24-hour keeps `15 14`; 12-hour becomes `3 14 P M`, and the minutes are
/// dropped on the hour rather than saying "3 00".
pub fn spoken_time(hour: u32, minute: u32, use_24_hour: bool) -> String {
    if use_24_hour {
        return format!("{hour:02} {minute:02}");
    }
    let period = if hour % 24 < 12 { "A M" } else { "P M" };
    let mut twelve = hour % 12;
    if twelve == 0 {
        twelve = 12;
    }
    if minute == 0 {
        format!("{twelve} {period}")
    } else {
        format!("{twelve} {minute:02} {period}")
    }
}

/// Resolve one anchor line: the custom text when set, otherwise the default,
/// with `@@@@` replaced by the full build time and every `@@` that is left by
/// just the hour. The `@@@@` is replaced first so its two `@@` halves are not
/// consumed by the hour rule.
pub fn anchor_line(
    custom: &str,
    default: String,
    hour: u32,
    minute: u32,
    use_24_hour: bool,
) -> String {
    let line = if custom.trim().is_empty() {
        default
    } else {
        custom.trim().to_string()
    };
    let full = line.replace(FULL_TIME_TOKEN, &spoken_time(hour, minute, use_24_hour));
    full.replace(HOUR_TOKEN, &spoken_hour(hour, use_24_hour))
}

/// Fetch every enabled feed and build the newscast show — the stories plus
/// the anchor's intro and outro — cached ~2 minutes like v1.0.0.
async fn preview_newscast(State(state): State<Arc<NewsState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let show_name = resolve_show_name(&settings.header);
    let now = Local::now();
    let intro = anchor_line(
        &settings.anchor_intro,
        default_intro(&show_name),
        now.hour(),
        now.minute(),
        settings.use_24_hour,
    );
    let outro = anchor_line(
        &settings.anchor_outro,
        default_outro(&show_name),
        now.hour(),
        now.minute(),
        settings.use_24_hour,
    );

    // The cache holds the fetched pool (extra stories when the rewrite is on,
    // so the writer has material); the show itself is the article count, then
    // woven into one broadcast when the option is set.
    let mut stories: Vec<Value> = cached_stories(&state, &settings)
        .await
        .into_iter()
        .take(settings.article_count.max(1) as usize)
        .collect();
    if settings.ai_rewrite {
        stories = crate::rewrite::rewrite_show(&state, &settings, &stories).await;
    }
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
    let stories = fetch_enabled_stories(state, settings).await;
    {
        let mut cache = state.headline_cache.lock().await;
        *cache = Some((key, Instant::now(), stories.clone()));
    }
    stories
}

/// Fetch every enabled feed's stories, uncached — the bulletin task uses this
/// so a run always sees the latest headlines, not the page's 2-minute cache.
pub(crate) async fn fetch_enabled_stories(
    state: &Arc<NewsState>,
    settings: &NewsSettings,
) -> Vec<Value> {
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
    fetch_stories(&state.http, &enabled, limit).await
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
            "title": clean(&title),
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
    // The feed's host, as the on-air attribution (the settings keep only the
    // feed url, so the host stands in for v1's feed name).
    feed_url
        .split("://")
        .nth(1)
        .unwrap_or(feed_url)
        .split('/')
        .next()
        .unwrap_or(feed_url)
        .to_string()
}

/// Trim HTML tags and collapse whitespace in a summary's text. Entities are
/// decoded first — an XML feed encodes tags as `&lt;b&gt;`, so undecoding after
/// stripping the literal markup would leave the encoded tags on screen.
fn clean(value: &str) -> String {
    let decoded = unescape(value);
    let mut text = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for ch in decoded.chars() {
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
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode HTML/XML entities — both the numeric forms (`&#039;`, `&#x27;`) and
/// the common named ones — before any tag stripping, so encoded apostrophes
/// and quotes cannot leak into the on-screen copy.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if let Some(end) = tail.find(';') {
            let name = &tail[1..end];
            let decoded = if let Some(number) = name.strip_prefix('#') {
                let digits = if let Some(hex) = number
                    .strip_prefix('x')
                    .or_else(|| number.strip_prefix('X'))
                {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    number.parse::<u32>().ok()
                };
                digits.and_then(char::from_u32).map(|c| c.to_string())
            } else {
                match name {
                    "amp" => Some("&".to_string()),
                    "lt" => Some("<".to_string()),
                    "gt" => Some(">".to_string()),
                    "quot" => Some("\"".to_string()),
                    "apos" | "rsquo" | "lsquo" => Some("'".to_string()),
                    "nbsp" => Some(" ".to_string()),
                    "rdquo" | "ldquo" => Some("\"".to_string()),
                    "hellip" => Some("…".to_string()),
                    "ndash" => Some("–".to_string()),
                    "mdash" => Some("—".to_string()),
                    "eacute" => Some("é".to_string()),
                    "Eacute" => Some("É".to_string()),
                    _ => None,
                }
            };
            if let Some(decoded) = decoded {
                out.push_str(&decoded);
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// The News page's bulletin status: the schedule settings and the ledger.
async fn bulletin_status(State(state): State<Arc<NewsState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    let ledger = crate::bulletin::ledger_snapshot(&state).await;
    Json(json!({ "bulletin": crate::bulletin::describe_ledger(&ledger, &settings) })).into_response()
}

/// The run request: `?force=` decides whether the minimum-new-stories gate is
/// bypassed. Defaults to forcing, so the News page's Build now keeps its
/// "build regardless" meaning; a scheduled-style run passes `force=false`.
#[derive(Deserialize)]
struct BulletinRunQuery {
    #[serde(default)]
    force: Option<bool>,
}

/// Run one bulletin build. `force=true` (default) always re-encodes; without
/// it the run goes through the same gate as a scheduled mark (once a day for
/// sure, otherwise only when enough stories are new).
async fn bulletin_run(
    State(state): State<Arc<NewsState>>,
    Query(query): Query<BulletinRunQuery>,
) -> Response {
    let force = query.force.unwrap_or(true);
    let root = state.config_root.clone();
    let outcome = crate::bulletin::run(&state, &root, force).await;
    Json(json!({ "outcome": outcome })).into_response()
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
    fn the_anchor_time_is_spelled_for_speech() {
        // A bare `@@` is just the hour. 12-hour keeps the letters apart so the
        // TTS says "A M", not "am".
        assert_eq!(spoken_hour(8, false), "8 A M");
        assert_eq!(spoken_hour(15, false), "3 P M");
        assert_eq!(spoken_hour(0, false), "12 A M");
        assert_eq!(spoken_hour(12, false), "12 P M");
        // 24-hour keeps the hour as the clock shows it.
        assert_eq!(spoken_hour(15, true), "15");
        assert_eq!(spoken_hour(6, true), "06");
        // `@@@@` is the full time: hour and minute, space-separated so the TTS
        // reads "ten fourteen", not "ten colon fourteen".
        assert_eq!(spoken_time(8, 0, false), "8 A M");
        assert_eq!(spoken_time(15, 0, false), "3 P M");
        assert_eq!(spoken_time(0, 0, false), "12 A M");
        assert_eq!(spoken_time(12, 0, false), "12 P M");
        assert_eq!(spoken_time(8, 30, false), "8 30 A M");
        assert_eq!(spoken_time(15, 0, true), "15 00");
        assert_eq!(spoken_time(6, 0, true), "06 00");
        assert_eq!(spoken_time(15, 14, true), "15 14");
    }

    #[test]
    fn the_build_time_fills_the_anchor_lines() {
        // The default lines carry the full-time token (`@@@@`): on the hour the
        // minutes are dropped, so 8:00 reads "8 A M" as before.
        let intro = anchor_line("", default_intro("FlowWire News"), 8, 0, false);
        assert_eq!(
            intro,
            "I'm Catherine Wolfe, And you're watching FlowWire News At 8 A M"
        );
        let outro = anchor_line("", default_outro("FlowWire News"), 15, 14, false);
        assert_eq!(outro, "I'm Catherine Wolfe, This was FlowWire News At 3 14 P M");
        // A bare `@@` says only the hour; `@@@@` says the full time.
        assert_eq!(
            anchor_line("Good evening, it's @@ now.", String::new(), 20, 12, false),
            "Good evening, it's 8 P M now."
        );
        assert_eq!(
            anchor_line("It's @@.", String::new(), 15, 12, true),
            "It's 15."
        );
        assert_eq!(
            anchor_line("It's @@@@.", String::new(), 15, 12, true),
            "It's 15 12."
        );
        // Both tokens in one line resolve independently.
        assert_eq!(
            anchor_line("The time is @@@@, the hour is @@.", String::new(), 15, 14, false),
            "The time is 3 14 P M, the hour is 3 P M."
        );
    }

    #[test]
    fn the_form_receives_the_default_anchor_script() {
        // The form pre-fills the intro/outro boxes with these, so they must
        // carry the full-time token and the show name.
        let settings = NewsSettings::default();
        let view = view(&settings);
        let options = view.get("options").expect("options");
        let intro = options["default_intro"].as_str().unwrap_or("");
        let outro = options["default_outro"].as_str().unwrap_or("");
        assert!(intro.contains("FlowWire News"), "intro names the show: {intro}");
        assert!(intro.contains(FULL_TIME_TOKEN), "intro keeps the time token: {intro}");
        assert!(outro.contains("FlowWire News"), "outro names the show: {outro}");
        assert!(outro.contains(FULL_TIME_TOKEN), "outro keeps the time token: {outro}");
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
        assert_eq!(stories[1]["summary"], "Whole story here.");
    }

    #[test]
    fn encoded_tags_are_stripped_from_summaries_and_titles() {
        let body = "<item><title><![CDATA[Big &lt;b&gt;sale&lt;/b&gt; today]]></title>\
            <description><![CDATA[<p>Now &lt;i&gt;everyone&lt;/i&gt; saves &amp; more.</p>]]></description></item>";
        let stories = extract_stories(body, "https://feed.example/news");
        assert_eq!(stories[0]["title"], "Big sale today", "no HTML left in the title");
        assert_eq!(stories[0]["summary"], "Now everyone saves & more.", "no HTML left in the summary");
    }

    #[test]
    fn numeric_entities_like_the_apostrophe_are_decoded() {
        let body = "<item><title><![CDATA[Tragic scene&#039;: 8 dead including children]]></title>\
            <description><![CDATA[Officials said it&#x27;s the worst &#39;in years&#39;.]]></description></item>";
        let stories = extract_stories(body, "https://feed.example/news");
        assert_eq!(
            stories[0]["title"],
            "Tragic scene': 8 dead including children",
            "&#039; becomes an apostrophe"
        );
        assert_eq!(
            stories[0]["summary"],
            "Officials said it's the worst 'in years'.",
            "&#x27; and &#39; become apostrophes"
        );
    }

    #[test]
    fn relative_images_resolve_against_the_feed() {
        let body = "<item><title>T</title><description><img src=\"/media/pic.png\"></description></item>";
        let stories = extract_stories(body, "https://feed.example/news/rss");
        assert_eq!(stories[0]["image"], "https://feed.example/media/pic.png");
    }
}