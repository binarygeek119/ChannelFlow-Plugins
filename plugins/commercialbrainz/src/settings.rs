//! The CommercialBrainz settings — where ad avails come from and how picky
//! the pool is. Mirrors 1.0.0's CommercialBrainzSettings: the server (base
//! URL + API token), the pool mode, how many results to sync, and the
//! allowed-content filters.

use serde::{Deserialize, Serialize};

/// The key the settings are stored under in the plugin's namespaced storage.
pub const SETTINGS_KEY: &str = "settings";

/// The public CommercialBrainz server.
pub const DEFAULT_BASE_URL: &str = "https://commercialbrainz.org";

/// Where the commercial pool is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolMode {
    JellyfinOnly,
    CommercialBrainzOnly,
    Both,
}

impl Default for PoolMode {
    fn default() -> Self {
        PoolMode::Both
    }
}

/// The whole CommercialBrainz settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommercialBrainzSettings {
    pub enabled: bool,
    pub base_url: String,
    pub api_token: String,
    pub pool_mode: PoolMode,
    pub max_sync_results: u32,
    pub min_year: Option<i32>,
    pub max_year: Option<i32>,
    pub decades: Vec<i32>,
    pub brands: Vec<String>,
    pub tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub genres: Vec<String>,
    pub networks: Vec<String>,
    pub channel_names: Vec<String>,
    pub allow_spoof: bool,
    pub allow_fake: bool,
    pub allow_real: bool,
    pub allow_ai_enhanced: bool,
    pub allow_late_night: bool,
    pub allow_adult_rated: bool,
    pub allow_banned: bool,
}

impl Default for CommercialBrainzSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: DEFAULT_BASE_URL.to_string(),
            api_token: String::new(),
            pool_mode: PoolMode::default(),
            max_sync_results: 500,
            min_year: None,
            max_year: None,
            decades: Vec::new(),
            brands: Vec::new(),
            tags: Vec::new(),
            exclude_tags: Vec::new(),
            genres: Vec::new(),
            networks: Vec::new(),
            channel_names: Vec::new(),
            allow_spoof: true,
            allow_fake: true,
            allow_real: true,
            allow_ai_enhanced: true,
            allow_late_night: true,
            allow_adult_rated: false,
            allow_banned: false,
        }
    }
}

#[derive(Debug)]
pub struct CommercialBrainzError(pub String);

impl std::fmt::Display for CommercialBrainzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CommercialBrainzError {}

impl CommercialBrainzSettings {
    /// Parse, normalise, and validate a stored or posted document.
    pub fn parse(value: &serde_json::Value) -> Result<Self, CommercialBrainzError> {
        let mut settings: CommercialBrainzSettings = serde_json::from_value(value.clone())
            .map_err(|error| {
                CommercialBrainzError(format!("commercialbrainz settings are not valid: {error}"))
            })?;
        settings.normalize();
        Ok(settings)
    }

    /// The base URL is trimmed and defaults; the result limit is clamped to
    /// the range the API supports (1..=500).
    pub fn normalize(&mut self) {
        self.base_url = normalize_base_url(&self.base_url);
        self.max_sync_results = self.max_sync_results.clamp(1, 500);
        trim_all(&mut self.brands);
        trim_all(&mut self.tags);
        trim_all(&mut self.exclude_tags);
        trim_all(&mut self.genres);
        trim_all(&mut self.networks);
        trim_all(&mut self.channel_names);
    }
}

fn normalize_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        value.to_string()
    }
}

fn trim_all(values: &mut Vec<String>) {
    values.retain(|value| !value.trim().is_empty());
    for value in values.iter_mut() {
        *value = value.trim().to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_the_public_server() {
        let settings = CommercialBrainzSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.base_url, DEFAULT_BASE_URL);
        assert_eq!(settings.pool_mode, PoolMode::Both);
        assert_eq!(settings.max_sync_results, 500);
    }

    #[test]
    fn normalises_url_and_clamps_results() {
        let value = serde_json::json!({
            "base_url": "https://example.com/",
            "max_sync_results": 100000,
        });
        let settings = CommercialBrainzSettings::parse(&value).expect("parses");
        assert_eq!(settings.base_url, "https://example.com");
        assert_eq!(settings.max_sync_results, 500);
    }

    #[test]
    fn trims_list_filters() {
        let value = serde_json::json!({ "brands": [" Coke ", "", "Pepsi"] });
        let settings = CommercialBrainzSettings::parse(&value).expect("parses");
        assert_eq!(settings.brands, ["Coke", "Pepsi"]);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = serde_json::json!({ "baseUrl": "https://x.example" });
        assert!(CommercialBrainzSettings::parse(&bad).is_err());
    }
}