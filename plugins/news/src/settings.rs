//! The News plugin settings — the FlowWire newscast: the header, how many
//! articles and how often to refresh, the TTS voice and engine, the anchor
//! intro/outro, the bulletin schedule, and the RSS feeds. Mirrors 1.0.0's
//! News tab.

use serde::{Deserialize, Serialize};

/// The key the settings are stored under in the plugin's namespaced storage.
pub const SETTINGS_KEY: &str = "settings";

/// One RSS feed, saved even when disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsFeed {
    pub url: String,
    #[serde(default)]
    pub enabled: bool,
}

/// The TTS engine used to read the stories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngine {
    Google,
    Ai,
}

impl Default for TtsEngine {
    fn default() -> Self {
        TtsEngine::Google
    }
}

/// The whole News settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NewsSettings {
    pub header: String,
    pub article_count: u32,
    pub refresh_minutes: u32,
    pub tts_voice: String,
    pub anchor_intro: String,
    pub anchor_outro: String,
    pub tts_enabled: bool,
    pub tts_engine: TtsEngine,
    pub ai_rewrite: bool,
    pub show_header: bool,
    pub headlines_only: bool,
    pub no_music: bool,
    pub bulletin_enabled: bool,
    pub minimum_new_stories: u32,
    pub feeds: Vec<NewsFeed>,
}

impl Default for NewsSettings {
    fn default() -> Self {
        Self {
            header: "FlowWire News".to_string(),
            article_count: 8,
            refresh_minutes: 10,
            tts_voice: "en-US".to_string(),
            anchor_intro: String::new(),
            anchor_outro: String::new(),
            tts_enabled: true,
            tts_engine: TtsEngine::default(),
            ai_rewrite: false,
            show_header: true,
            headlines_only: false,
            no_music: false,
            bulletin_enabled: true,
            minimum_new_stories: 1,
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
    pub fn parse(value: &serde_json::Value) -> Result<Self, NewsError> {
        let mut settings: NewsSettings = serde_json::from_value(value.clone()).map_err(|error| {
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
        self.article_count = self.article_count.clamp(1, 30);
        self.refresh_minutes = self.refresh_minutes.clamp(2, 120);
        self.minimum_new_stories = self.minimum_new_stories.clamp(1, 30);
        self.feeds.retain(|feed| !feed.url.trim().is_empty());
        for feed in self.feeds.iter_mut() {
            feed.url = feed.url.trim().to_string();
        }
    }
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
}