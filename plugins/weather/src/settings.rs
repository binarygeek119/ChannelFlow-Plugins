//! The Weather plugin settings — the WeatherStar look, the weather source,
//! the default location, units, and which screens play in the loop. Mirrors
//! 1.0.0's Weather tab.

use serde::{Deserialize, Serialize};

/// The key the settings are stored under in the plugin's namespaced storage.
pub const SETTINGS_KEY: &str = "settings";

/// The WeatherStar look weather channels render with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StarVariant {
    Ws4kp,
    Ws3kp,
}

impl Default for StarVariant {
    fn default() -> Self {
        StarVariant::Ws4kp
    }
}

/// Where the forecast comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// NOAA in the US, Open-Meteo worldwide.
    Auto,
    /// United States (NOAA).
    Us,
    /// World (Open-Meteo).
    World,
}

impl Default for Source {
    fn default() -> Self {
        Source::Auto
    }
}

/// Display units for the weather screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    Us,
    Si,
}

impl Default for Units {
    fn default() -> Self {
        Units::Us
    }
}

/// Every screen the WeatherStar loop can show.
pub const ALL_SCREENS: [&str; 12] = [
    "hazards",
    "current",
    "latest_observations",
    "hourly",
    "hourly_graph",
    "travel",
    "regional",
    "local",
    "extended",
    "almanac",
    "spc_outlook",
    "radar",
];

/// The whole Weather settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherSettings {
    pub weatherstar_variant: StarVariant,
    pub source: Source,
    /// ZIP, city, or `latitude,longitude`.
    pub default_location: String,
    pub units: Units,
    /// Use the wide layout for 16:9 weather channels.
    pub auto_wide_169: bool,
    /// The screens enabled in the loop, by id.
    pub screens: Vec<String>,
}

impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            weatherstar_variant: StarVariant::default(),
            source: Source::default(),
            default_location: String::new(),
            units: Units::default(),
            auto_wide_169: true,
            screens: ALL_SCREENS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug)]
pub struct WeatherError(pub String);

impl std::fmt::Display for WeatherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WeatherError {}

impl WeatherSettings {
    /// Parse a stored or posted document, keeping only known screens.
    pub fn parse(value: &serde_json::Value) -> Result<Self, WeatherError> {
        let mut settings: WeatherSettings = serde_json::from_value(value.clone()).map_err(
            |error| WeatherError(format!("weather settings are not valid: {error}")),
        )?;
        settings.screens.retain(|screen| ALL_SCREENS.contains(&screen.as_str()));
        settings.default_location = settings.default_location.trim().to_string();
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_render_all_screens() {
        let settings = WeatherSettings::default();
        assert_eq!(settings.weatherstar_variant, StarVariant::Ws4kp);
        assert_eq!(settings.source, Source::Auto);
        assert_eq!(settings.units, Units::Us);
        assert_eq!(settings.screens.len(), ALL_SCREENS.len());
    }

    #[test]
    fn parses_and_filters_unknown_screens() {
        let value = serde_json::json!({
            "weatherstar_variant": "ws3kp",
            "source": "world",
            "units": "si",
            "auto_wide_169": false,
            "screens": ["current", "radar", "not-a-screen"]
        });
        let settings = WeatherSettings::parse(&value).expect("parses");
        assert_eq!(settings.weatherstar_variant, StarVariant::Ws3kp);
        assert_eq!(settings.screens, ["current", "radar"]);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = serde_json::json!({ "weatherStarVariant": "ws4kp" });
        assert!(WeatherSettings::parse(&bad).is_err());
    }
}