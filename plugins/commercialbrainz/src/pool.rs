//! Pulling the commercial pool from the CommercialBrainz server and applying
//! the saved filters, the way v1.0.0's CommercialBrainzClient + FilterService
//! did: page through `api/v1/browse/videos` with bearer auth, then accept only
//! the commercials that match the year/decade/brand/tag/network/channel and
//! allow-content settings.
//!
//! The spoof/fake/late-night/adult classification is a keyword-based stand-in
//! for v1's full metadata classifier; every other filter follows v1.

use serde_json::{json, Value};

use crate::settings::CommercialBrainzSettings;

/// One browse page from the commercial server.
pub struct BrowsePage {
    pub items: Vec<Value>,
    pub total: i64,
}

/// Fetch `{base}/api/v1/browse/videos?offset=&limit=` with the API token.
pub async fn browse(
    http: &reqwest::Client,
    settings: &CommercialBrainzSettings,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    let url = format!(
        "{}/api/v1/browse/videos?offset={offset}&limit={limit}",
        settings.base_url
    );
    let request = http.get(&url).header("Accept", "application/json");
    let request = if settings.api_token.trim().is_empty() {
        request
    } else {
        request.header("Authorization", format!("Bearer {}", settings.api_token.trim()))
    };
    let response = request
        .send()
        .await
        .map_err(|_| "could not reach the CommercialBrainz server".to_string())?;
    if !response.status().is_success() {
        return Err(format!("the server answered {}", response.status()));
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|_| "the server reply was unreadable".to_string())?;
    Ok(BrowsePage {
        items: body
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        total: body.get("total").and_then(Value::as_i64).unwrap_or_default(),
    })
}

/// Whether the saved filters accept a video, with the first reason it failed
/// (the page can say why so many were filtered out).
pub fn filter(settings: &CommercialBrainzSettings, video: &Value) -> Result<(), &'static str> {
    if video.get("youtube_id").and_then(Value::as_str).map_or(true, str::is_empty)
        && video.get("youtube_url").and_then(Value::as_str).map_or(false, str::is_empty)
    {
        return Err("no YouTube link");
    }
    let title = get_title(video);
    let year = get_year(video);
    let decade = year.map(|y| y / 10 * 10);
    let brand = get_brand(video).to_string();
    let tags = get_tags(video);

    if let Some(min) = settings.min_year {
        if year.is_none_or(|y| y < min as i64) {
            return Err("before the min year");
        }
    }
    if let Some(max) = settings.max_year {
        if year.is_none_or(|y| y > max as i64) {
            return Err("after the max year");
        }
    }
    if !settings.decades.is_empty() {
        if decade.is_none_or(|d| !settings.decades.iter().any(|want| *want as i64 == d)) {
            return Err("wrong decade");
        }
    }
    if !settings.brands.is_empty()
        && !settings
            .brands
            .iter()
            .any(|want| matches_token(&brand, want) || matches_token(&title, want))
    {
        return Err("brand not requested");
    }
    if !settings.tags.is_empty()
        && !settings
            .tags
            .iter()
            .any(|want| tags.iter().any(|tag| matches_token(tag, want)))
    {
        return Err("tag not requested");
    }
    if !settings.exclude_tags.is_empty()
        && settings
            .exclude_tags
            .iter()
            .any(|excluded| tags.iter().any(|tag| matches_token(tag, excluded)))
    {
        return Err("excluded tag present");
    }
    if !settings.networks.is_empty() {
        let network = get_network(video);
        if !settings
            .networks
            .iter()
            .any(|want| matches_token(network, want))
        {
            return Err("network not requested");
        }
    }
    if !settings.channel_names.is_empty() {
        let channel = get_channel(video);
        if !settings
            .channel_names
            .iter()
            .any(|want| matches_token(channel, want))
        {
            return Err("channel not requested");
        }
    }

    // A keyword stand-in for v1's classifier: adult / late-night / spoof /
    // fake come from the title, tags, channel, and visibility field.
    let blob = format!(
        "{title} {} {} {}",
        brand,
        get_channel(video),
        get_network(video)
    )
    .to_lowercase();
    let tags_blob = tags.join(" ").to_lowercase();
    let is_adult = blob.contains("adult") || blob.contains("nsfw") || tags_blob.contains("adult");
    let is_late_night = blob.contains("late night") || blob.contains("adult swim");
    let is_spoof =
        tags_blob.contains("spoof") || tags_blob.contains("parody") || blob.contains("spoof");
    let is_fake = tags_blob.contains("fake") || blob.contains("parody") || blob.contains("fake ad");

    if !settings.allow_adult_rated && is_adult {
        return Err("adult rated");
    }
    if !settings.allow_late_night && is_late_night {
        return Err("late night");
    }
    if !settings.allow_spoof && is_spoof {
        return Err("spoof");
    }
    if !settings.allow_fake && is_fake {
        return Err("fake");
    }
    Ok(())
}

/// The item shaped for the page: id, YouTube link + thumbnail, title,
/// attribution, and the classification the filters saw.
pub fn item_view(video: &Value, base_url: &str) -> Value {
    let youtube_id = video
        .get("youtube_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|id| !id.is_empty());
    let youtube_url = video
        .get("youtube_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|url| !url.is_empty())
        .or_else(|| youtube_id.as_ref().map(|id| format!("https://www.youtube.com/watch?v={id}")));
    let thumbnail = video
        .get("thumbnail_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|url| !url.is_empty())
        .or_else(|| youtube_id.as_ref().map(|id| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")))
        .or_else(|| {
            video
                .get("metadata")
                .and_then(|meta| meta.get("youtube_thumbnail"))
                .and_then(Value::as_str)
                .map(|url| resolve_media(base_url, url))
        });
    json!({
        "sbid": video.get("sbid"),
        "youtube_id": youtube_id,
        "youtube_url": youtube_url,
        "thumbnail_url": thumbnail,
        "title": get_title(video),
        "brand": get_brand(video),
        "channel": get_channel(video),
        "network": get_network(video),
        "duration_ms": video.get("duration_ms"),
        "year": get_year(video),
        "tags": get_tags(video),
        "visibility": video.get("visibility"),
    })
}

fn get_title(video: &Value) -> String {
    if let Some(title) = video.pointer("/commercial/title").and_then(Value::as_str) {
        if !title.trim().is_empty() {
            return title.trim().to_string();
        }
    }
    if let Some(title) = video.get("commercial_title").and_then(Value::as_str) {
        if !title.trim().is_empty() {
            return title.trim().to_string();
        }
    }
    if let Some(title) = video
        .pointer("/metadata/youtube_title")
        .and_then(Value::as_str)
    {
        if !title.trim().is_empty() {
            return title.trim().to_string();
        }
    }
    video.get("channel_name").and_then(Value::as_str).unwrap_or("").to_string()
}

fn get_brand(video: &Value) -> &str {
    video
        .pointer("/advertiser/name")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn get_channel(video: &Value) -> &str {
    video.get("channel_name").and_then(Value::as_str).unwrap_or("")
}

fn get_network(video: &Value) -> &str {
    video.get("network").and_then(Value::as_str).unwrap_or("")
}

fn get_tags(video: &Value) -> Vec<String> {
    video
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A year from the commercial row, the title, or a 4-digit tag.
fn get_year(video: &Value) -> Option<i64> {
    if let Some(year) = video.pointer("/commercial/year").and_then(Value::as_i64) {
        if (1900..=2099).contains(&year) {
            return Some(year);
        }
    }
    for candidate in [
        video.pointer("/commercial/title").and_then(Value::as_str),
        video.get("commercial_title").and_then(Value::as_str),
        video.pointer("/metadata/youtube_title").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(year) = year_in(candidate) {
            return Some(year);
        }
    }
    for tag in get_tags(video) {
        if tag.trim().len() == 4 {
            if let Some(year) = year_in(&tag) {
                return Some(year);
            }
        }
    }
    None
}

fn year_in(text: &str) -> Option<i64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|part| part.len() == 4)
        .find_map(|part| part.parse::<i64>().ok().filter(|year| (1900..=2099).contains(year)))
}

fn matches_token(value: &str, want: &str) -> bool {
    let want = want.trim().to_lowercase();
    value.to_lowercase().contains(&want)
}

fn resolve_media(base_url: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    format!("{}/{}", base_url, url.trim_start_matches('/'))
}