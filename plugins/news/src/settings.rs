//! The News plugin settings — the FlowWire newscast: the header, how many
//! articles and how often to refresh, the TTS voice, the anchor intro/outro,
//! the bulletin schedule, and the RSS feeds. Mirrors 1.0.0's News tab.
//!
//! The voice that reads the show is chosen here: the AI page's TTS provider
//! (the core hands the plugin a TTS provider) or Google TTS. The language kept
//! here is the Google voice, and the fallback when the AI provider fails.

use serde::{Deserialize, Serialize};

/// The key the settings are stored under in the plugin's namespaced storage.
pub const SETTINGS_KEY: &str = "settings";

/// The old AI-TTS fields, dropped before parsing so a document saved by an
/// earlier version still loads (`deny_unknown_fields` would otherwise reject
/// it). The AI page owns the endpoint, key, model, and voice now. `tts_engine`
/// is kept: it is a real setting again (AI vs Google).
const LEGACY_TTS_FIELDS: [&str; 4] = [
    "ai_tts_base_url",
    "ai_tts_api_key",
    "ai_tts_model",
    "ai_tts_voice",
];

/// Which voice reads the show: the AI page's TTS provider, or Google TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngine {
    Ai,
    Google,
}

impl Default for TtsEngine {
    fn default() -> Self {
        // Prefer the AI page's provider when one is set (falling back to Google
        // if it fails), matching how the show has always been read.
        TtsEngine::Ai
    }
}

/// One RSS feed, saved even when disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsFeed {
    pub url: String,
    #[serde(default)]
    pub enabled: bool,
}

/// The whole News settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NewsSettings {
    pub header: String,
    pub article_count: u32,
    pub refresh_minutes: u32,
    /// The Google TTS language (e.g. `en-US`), used when the engine is Google
    /// (or as the fallback when the AI provider fails).
    pub tts_voice: String,
    /// Which voice reads the show: the AI page's provider or Google TTS.
    pub tts_engine: TtsEngine,
    pub anchor_intro: String,
    pub anchor_outro: String,
    /// Speak the build time as 24-hour (`15:00`) instead of 12-hour (`3 P M`).
    pub use_24_hour: bool,
    pub tts_enabled: bool,
    pub ai_rewrite: bool,
    pub show_header: bool,
    pub headlines_only: bool,
    pub no_music: bool,
    pub bulletin_enabled: bool,
    pub minimum_new_stories: u32,
    /// The local-clock hours a bulletin may build, comma-separated. 1.0.0
    /// rebuilt every six hours; ChannelFlow 2.0.0 runs at these six marks.
    pub bulletin_times: String,
    pub feeds: Vec<NewsFeed>,
}

impl Default for NewsSettings {
    fn default() -> Self {
        Self {
            header: "FlowWire News".to_string(),
            article_count: 8,
            refresh_minutes: 10,
            tts_voice: "en-US".to_string(),
            tts_engine: TtsEngine::default(),
            anchor_intro: String::new(),
            anchor_outro: String::new(),
            use_24_hour: false,
            tts_enabled: true,
            ai_rewrite: false,
            show_header: true,
            headlines_only: false,
            no_music: false,
            bulletin_enabled: true,
            minimum_new_stories: 1,
            bulletin_times: "6,10,14,18,20,22".to_string(),
            feeds: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct NewsError(pub String);

impl std::fmt::Display for NewsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NewsError {}

impl NewsSettings {
    /// Parse a stored or posted document, normalising the bounds and feeds.
    /// The retired AI-TTS fields are discarded first, so a document saved by
    /// an earlier version still loads.
    pub fn parse(value: &serde_json::Value) -> Result<Self, NewsError> {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            for field in LEGACY_TTS_FIELDS {
                object.remove(field);
            }
        }
        let mut settings: NewsSettings = serde_json::from_value(value).map_err(|error| {
            NewsError(format!("news settings are not valid: {error}"))
        })?;
        settings.normalize();
        Ok(settings)
    }

    /// Clamp the numeric bounds, trim the free-text fields, and drop empty
    /// or URL-less feeds.
    pub fn normalize(&mut self) {
        self.header = self.header.trim().to_string();
        self.anchor_intro = self.anchor_intro.trim().to_string();
        self.anchor_outro = self.anchor_outro.trim().to_string();
        self.tts_voice = self.tts_voice.trim().to_string();
        self.article_count = self.article_count.clamp(1, 30);
        self.refresh_minutes = self.refresh_minutes.clamp(2, 120);
        self.minimum_new_stories = self.minimum_new_stories.clamp(1, 30);
        self.bulletin_times = normalized_times(&self.bulletin_times);
        self.feeds.retain(|feed| !feed.url.trim().is_empty());
        for feed in self.feeds.iter_mut() {
            feed.url = feed.url.trim().to_string();
        }
    }

    /// The local-clock hours a bulletin may build.
    pub fn bulletin_hours(&self) -> Vec<u32> {
        self.bulletin_times
            .split(',')
            .filter_map(|part| part.trim().parse::<u32>().ok())
            .collect()
    }
}

/// A comma-separated hour list, sorted, de-duplicated, and kept to 0..=23.
/// An empty result falls back to the six default marks.
fn normalized_times(value: &str) -> String {
    let mut hours: Vec<u32> = value
        .split(',')
        .filter_map(|part| part.trim().parse::<u32>().ok())
        .filter(|hour| *hour <= 23)
        .collect();
    hours.sort_unstable();
    hours.dedup();
    if hours.is_empty() {
        return "6,10,14,18,20,22".to_string();
    }
    hours
        .iter()
        .map(|hour| hour.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_flowwire() {
        let settings = NewsSettings::default();
        assert_eq!(settings.header, "FlowWire News");
        assert_eq!(settings.article_count, 8);
        assert_eq!(settings.tts_voice, "en-US");
        assert!(settings.tts_enabled);
    }

    #[test]
    fn clamps_and_trims() {
        let value = serde_json::json!({
            "header": "  The Evening News  ",
            "article_count": 999,
            "refresh_minutes": 1,
            "feeds": [{ "url": " https://example.com/rss ", "enabled": true }, { "url": " ", "enabled": true }]
        });
        let settings = NewsSettings::parse(&value).expect("parses");
        assert_eq!(settings.header, "The Evening News");
        assert_eq!(settings.article_count, 30);
        assert_eq!(settings.refresh_minutes, 2);
        assert_eq!(settings.feeds.len(), 1);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = serde_json::json!({ "articleCount": 8 });
        assert!(NewsSettings::parse(&bad).is_err());
    }

    /// A document saved before the AI-TTS fields were retired still loads: the
    /// retired `ai_tts_*` fields are dropped, but `tts_engine` is a real setting
    /// again and is honoured.
    #[test]
    fn drops_the_retired_ai_tts_fields() {
        let old = serde_json::json!({
            "header": "FlowWire News",
            "tts_engine": "google",
            "ai_tts_base_url": "https://example.com/v1",
            "ai_tts_api_key": "secret",
            "ai_tts_model": "kokoro",
            "ai_tts_voice": "af_sarah",
            "feeds": [{ "url": "https://example.com/rss", "enabled": true }]
        });
        let settings = NewsSettings::parse(&old).expect("the old document still loads");
        assert_eq!(settings.header, "FlowWire News");
        assert_eq!(settings.feeds.len(), 1);
        assert_eq!(settings.tts_engine, TtsEngine::Google);
        let round_trip = serde_json::to_value(&settings).expect("serializes");
        assert!(round_trip.get("ai_tts_base_url").is_none());
        assert!(round_trip.get("ai_tts_api_key").is_none());
        assert_eq!(round_trip["tts_engine"], "google");
    }

    #[test]
    fn the_tts_engine_defaults_to_the_ai_provider() {
        // A document with no engine keeps the long-standing behaviour: use the
        // AI page's provider, falling back to Google.
        let settings = NewsSettings::parse(&serde_json::json!({ "header": "FlowWire News" }))
            .expect("loads");
        assert_eq!(settings.tts_engine, TtsEngine::Ai);
    }
}
