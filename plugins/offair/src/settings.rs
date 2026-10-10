//! The Off Air settings — what plays when a channel has no scheduled media,
//! when playback fails, or when a weather capture errors. Mirrors 1.0.0's EBS
//! settings: video and audio are chosen independently.

use serde::{Deserialize, Serialize};

/// The key the settings are stored under in the plugin's namespaced storage.
pub const SETTINGS_KEY: &str = "settings";

/// Off-air video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    /// A full-screen slate image (stock or a custom upload).
    SlateImage,
    /// SMPTE color bars.
    ColorBars,
    /// TV static / snow.
    Static,
}

impl Default for DisplayMode {
    fn default() -> Self {
        DisplayMode::SlateImage
    }
}

/// Which stock slate to prefer when the video mode is a slate image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlateVariant {
    /// The USA off-air slate.
    Usa,
    /// The world off-air slate (1.0.0 stored this as "International").
    International,
}

impl Default for SlateVariant {
    fn default() -> Self {
        SlateVariant::Usa
    }
}

/// Off-air audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    /// A random track from the chosen music source.
    BackgroundMusic,
    /// Generated white noise.
    WhiteNoise,
    /// A silent stereo track.
    Silence,
    /// A repeating alert beep.
    BeepTone,
}

impl Default for AudioMode {
    fn default() -> Self {
        AudioMode::BackgroundMusic
    }
}

/// Where off-air background music comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSource {
    /// A random track from every music library.
    AllMusicLibraries,
    /// A random track from one named library.
    NamedLibrary,
    /// Local ChannelFlow music packs.
    LocalPacks,
}

impl Default for MusicSource {
    fn default() -> Self {
        MusicSource::AllMusicLibraries
    }
}

/// The whole Off Air settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OffAirSettings {
    pub display_mode: DisplayMode,
    pub slate_variant: SlateVariant,
    pub audio_mode: AudioMode,
    pub music_source: MusicSource,
    /// The chosen library's display name, when `music_source` is a named one.
    pub music_library_name: String,
    /// The chosen library's stable id, when `music_source` is a named one.
    pub music_library_id: String,
}

impl Default for OffAirSettings {
    fn default() -> Self {
        Self {
            display_mode: DisplayMode::default(),
            slate_variant: SlateVariant::default(),
            audio_mode: AudioMode::default(),
            music_source: MusicSource::default(),
            music_library_name: String::new(),
            music_library_id: String::new(),
        }
    }
}

#[derive(Debug)]
pub struct OffAirError(pub String);

impl std::fmt::Display for OffAirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OffAirError {}

impl OffAirSettings {
    /// Parse and validate a stored or posted document.
    pub fn parse(value: &serde_json::Value) -> Result<Self, OffAirError> {
        let settings: OffAirSettings = serde_json::from_value(value.clone())
            .map_err(|error| OffAirError(format!("off-air settings are not valid: {error}")))?;
        settings.validate()?;
        Ok(settings)
    }

    /// A named music library has to be named: background music from "a
    /// library" with no library would silently fall back to nothing.
    pub fn validate(&self) -> Result<(), OffAirError> {
        if self.audio_mode == AudioMode::BackgroundMusic
            && self.music_source == MusicSource::NamedLibrary
            && self.music_library_name.trim().is_empty()
            && self.music_library_id.trim().is_empty()
        {
            return Err(OffAirError(
                "choose a music library, or pick another music source".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_slate_and_background_music() {
        let settings = OffAirSettings::default();
        assert_eq!(settings.display_mode, DisplayMode::SlateImage);
        assert_eq!(settings.audio_mode, AudioMode::BackgroundMusic);
        assert_eq!(settings.music_source, MusicSource::AllMusicLibraries);
    }

    #[test]
    fn round_trips_and_validates() {
        let value = serde_json::json!({
            "display_mode": "color_bars",
            "slate_variant": "international",
            "audio_mode": "beep_tone",
            "music_source": "local_packs"
        });
        let settings = OffAirSettings::parse(&value).expect("parses");
        assert_eq!(settings.display_mode, DisplayMode::ColorBars);
        assert_eq!(settings.audio_mode, AudioMode::BeepTone);

        let named = serde_json::json!({ "audio_mode": "background_music", "music_source": "named_library" });
        assert!(OffAirSettings::parse(&named).is_err(), "needs a library");

        let named = serde_json::json!({ "music_source": "named_library", "music_library_name": "Chill" });
        assert!(OffAirSettings::parse(&named).is_ok());
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = serde_json::json!({ "displayMode": "color_bars" });
        assert!(OffAirSettings::parse(&bad).is_err());
    }
}