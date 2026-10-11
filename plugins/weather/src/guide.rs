//! Build the day's forecast into the core guide for the WeatherStar channel.
//!
//! The Weather plugin's daily task fetches the forecast once and writes one
//! guide block per hour for the day onto every WeatherStar channel, so the TV
//! Guide shows the forecast. Each block, like the live channel itself, opens
//! the WeatherStar feed — clicking one watches the channel, which renders the
//! current conditions.

use std::sync::Arc;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};
use serde_json::{json, Value};

use crate::forecast;
use crate::settings::{StarVariant, WeatherSettings};
use crate::WeatherState;

/// The plugin's storage key for the last daily build date.
const BUILD_KEY: &str = "guide-last-build";

/// The local date the guide was last built, if any.
pub async fn last_build_date(state: &Arc<WeatherState>) -> Option<String> {
    match state.storage.get(BUILD_KEY).await {
        Ok(Some(value)) => value.as_str().map(str::to_string),
        _ => None,
    }
}

/// Record that the guide was built today.
pub async fn record_build(state: &Arc<WeatherState>) {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let _ = state
        .storage
        .set(BUILD_KEY, &serde_json::Value::String(today))
        .await;
}

/// Whether a channel is a WeatherStar one. The presets are numbered 126
/// (legacy) and 119.5 (modern), but the channel may be renamed, so a name or
/// number match both count.
pub fn is_weather_channel(name: &str, number: f64) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("weatherstar")
        || lower.contains("weather")
        || number == 126.0
        || (number - 119.5).abs() < 1e-9
}

/// Build and store the day's forecast guide blocks. Returns how many program
/// rows were written (across every WeatherStar channel).
pub async fn build(state: &Arc<WeatherState>, force: bool) -> Result<usize, String> {
    let settings = state.settings.lock().await.clone();
    if !settings.guide_enabled && !force {
        return Ok(0);
    }
    if settings.default_location.trim().is_empty() {
        return Err("set a location first".to_string());
    }

    let place = forecast::geocode(&state.http, &settings.default_location).await?;
    let snapshot = forecast::snapshot(&state.http, &place, settings.source, settings.units).await?;

    let channels = state.core.channels().await.map_err(|error| error.to_string())?;
    let weather: Vec<_> = channels
        .into_iter()
        .filter(|channel| channel.enabled && is_weather_channel(&channel.name, channel.number))
        .collect();
    if weather.is_empty() {
        return Err("no WeatherStar channel found".to_string());
    }

    let programs = build_programs(&snapshot, &settings);
    let mut written = 0;
    for channel in &weather {
        state
            .core
            .set_channel_guide(&channel.id, programs.clone())
            .await
            .map_err(|error| error.to_string())?;
        written += programs.len();
    }
    Ok(written)
}

/// The 24 hourly blocks for the current local day, each carrying the day's
/// forecast so the guide reads like a weather schedule.
pub fn build_programs(snapshot: &Value, settings: &WeatherSettings) -> Vec<Value> {
    let current = &snapshot["current"];
    let condition = current["condition"].as_str().unwrap_or("Weather");
    let temp = current["temperature"].as_f64();
    let units = snapshot["units"]["temperature"].as_str().unwrap_or("");
    let today = snapshot["daily"]
        .as_array()
        .and_then(|days| days.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let high = today["max"].as_f64();
    let low = today["min"].as_f64();
    let day_condition = today["condition"].as_str().unwrap_or(condition);

    let variant = match settings.weatherstar_variant {
        StarVariant::Ws4kp => "WeatherStar 4000",
        StarVariant::Ws3kp => "WeatherStar 3000",
    };
    let summary = format!(
        "Now {}{units} {condition}. Today {day_condition}, high {}{units} / low {}{units}. Live {variant}.",
        fmt_temp(temp),
        fmt_temp(high),
        fmt_temp(low),
    );

    let date = Local::now().date_naive();
    let title = format!("{day_condition} {}·{}", fmt_temp(high), fmt_temp(low));
    let mut programs = Vec::with_capacity(24);
    for hour in 0..24u32 {
        let Some(start) = local_hour(date, hour) else {
            continue;
        };
        let finish = start + Duration::hours(1);
        programs.push(json!({
            "id": format!("weather-{}-{hour:02}", date.format("%Y%m%d")),
            "start": start.to_rfc3339(),
            "finish": finish.to_rfc3339(),
            "title": title,
            "sub_title": variant,
            "description": summary,
            "categories": ["Weather"],
            "year": date.year(),
        }));
    }
    programs
}

fn fmt_temp(value: Option<f64>) -> String {
    value.map(|t| format!("{t:.0}")).unwrap_or_else(|| "--".to_string())
}

/// The local instant at `hour:00` on `date`, tolerating DST gaps.
fn local_hour(date: NaiveDate, hour: u32) -> Option<chrono::DateTime<Local>> {
    let naive = date.and_hms_opt(hour, 0, 0)?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(value) => Some(value),
        chrono::LocalResult::Ambiguous(value, _) => Some(value),
        chrono::LocalResult::None => None,
    }
}

/// Whether the daily build time has arrived.
pub fn due(settings: &WeatherSettings, now: chrono::DateTime<Local>, last_built_date: Option<&str>) -> bool {
    let Some((hour, minute)) = parse_time(&settings.guide_build_time) else {
        return false;
    };
    now.hour() == hour
        && now.minute() == minute
        && last_built_date != Some(now.format("%Y-%m-%d").to_string().as_str())
}

fn parse_time(value: &str) -> Option<(u32, u32)> {
    let (hour, minute) = value.split_once(':')?;
    let hour: u32 = hour.trim().parse().ok()?;
    let minute: u32 = minute.trim().parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weatherstar_channels_are_recognised() {
        assert!(is_weather_channel("WeatherStar4000", 126.0));
        assert!(is_weather_channel("My Weather", 88.0));
        assert!(is_weather_channel("Channel", 119.5));
        assert!(!is_weather_channel("Movie Night", 5.0));
    }

    #[test]
    fn a_day_builds_twenty_four_hourly_blocks() {
        let snapshot = json!({
            "current": { "condition": "Rain", "temperature": 61.0 },
            "units": { "temperature": "°F" },
            "daily": [{ "condition": "Rain", "max": 66.0, "min": 52.0 }]
        });
        let settings = WeatherSettings::default();
        let programs = build_programs(&snapshot, &settings);
        assert_eq!(programs.len(), 24);
        assert!(programs[0]["start"].as_str().unwrap().contains('T'));
        assert_eq!(programs[0]["sub_title"], "WeatherStar 4000");
        assert!(programs[0]["title"].as_str().unwrap().contains("Rain"));
    }

    #[test]
    fn the_daily_build_fires_once_at_its_time() {
        use chrono::TimeZone;
        let settings = WeatherSettings::default();
        let now = Local.with_ymd_and_hms(2026, 10, 11, 0, 5, 0).unwrap();
        assert!(due(&settings, now, None), "00:05 builds when not built today");
        assert!(!due(&settings, now, Some("2026-10-11")), "but only once a day");
        let later = Local.with_ymd_and_hms(2026, 10, 11, 0, 6, 0).unwrap();
        assert!(!due(&settings, later, None), "not at 00:06");
    }
}
