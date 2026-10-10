//! The Emergency Broadcast System settings — how weather alerts splice into
//! programming. Mirrors 1.0.0's alert-overlay settings: whether alerts
//! display at all, and when they cut in, how often and for how long.

use serde::{Deserialize, Serialize};

/// The key the settings are stored under in the plugin's namespaced storage.
pub const SETTINGS_KEY: &str = "settings";

/// How an active alert overlays the current program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertDisplay {
    /// No alert overlay.
    Off,
    /// Switch to the alerts screen every so often, keeping the show audio low
    /// under the attention/end tones.
    #[serde(rename = "cutin")]
    CutIn,
    /// Scrolling alert text at the bottom over the current program.
    Ticker,
}

impl Default for AlertDisplay {
    fn default() -> Self {
        AlertDisplay::Off
    }
}

/// The whole Emergency Broadcast System settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmergencySettings {
    pub alert_display: AlertDisplay,
    /// How often a cut-in alert switches to the alerts screen, in minutes.
    pub cutin_interval_minutes: u32,
    /// How long the alerts screen stays up, in seconds.
    pub cutin_duration_seconds: u32,
}

impl Default for EmergencySettings {
    fn default() -> Self {
        Self {
            alert_display: AlertDisplay::default(),
            cutin_interval_minutes: 15,
            cutin_duration_seconds: 20,
        }
    }
}

#[derive(Debug)]
pub struct EmergencyError(pub String);

impl std::fmt::Display for EmergencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EmergencyError {}

impl EmergencySettings {
    /// Parse and validate a stored or posted document.
    pub fn parse(value: &serde_json::Value) -> Result<Self, EmergencyError> {
        let settings: EmergencySettings = serde_json::from_value(value.clone()).map_err(
            |error| EmergencyError(format!("emergency settings are not valid: {error}")),
        )?;
        settings.validate()?;
        Ok(settings)
    }

    /// The cut-in interval and duration have to stay in sane bounds.
    pub fn validate(&self) -> Result<(), EmergencyError> {
        if self.alert_display == AlertDisplay::CutIn {
            if !(1..=180).contains(&self.cutin_interval_minutes) {
                return Err(EmergencyError(
                    "cut-in interval must be between 1 and 180 minutes".to_string(),
                ));
            }
            if !(5..=120).contains(&self.cutin_duration_seconds) {
                return Err(EmergencyError(
                    "cut-in duration must be between 5 and 120 seconds".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        let settings = EmergencySettings::default();
        assert_eq!(settings.alert_display, AlertDisplay::Off);
        assert_eq!(settings.cutin_interval_minutes, 15);
        assert_eq!(settings.cutin_duration_seconds, 20);
    }

    #[test]
    fn parses_and_validates() {
        let value = serde_json::json!({
            "alert_display": "cutin",
            "cutin_interval_minutes": 30,
            "cutin_duration_seconds": 45,
        });
        let settings = EmergencySettings::parse(&value).expect("parses");
        assert_eq!(settings.alert_display, AlertDisplay::CutIn);

        let bad = serde_json::json!({ "alert_display": "cutin", "cutin_duration_seconds": 3 });
        assert!(EmergencySettings::parse(&bad).is_err(), "below the 5s floor");
    }

    #[test]
    fn accepts_ticker_without_bounds() {
        let value = serde_json::json!({ "alert_display": "ticker" });
        assert!(EmergencySettings::parse(&value).is_ok());
    }
}