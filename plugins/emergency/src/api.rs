//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.emergency`, so the Emergency Broadcast
//! System page's calls to `GET /`, `PUT /`, and `POST /test` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::settings::{AlertDisplay, EmergencySettings, SETTINGS_KEY};
use crate::EmergencyState;

pub fn router(state: Arc<EmergencyState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/test", get(test_alerts))
        .route("/alerts", get(current_alerts))
        .with_state(state)
}

fn view(settings: &EmergencySettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "alert_displays": ["off", "cutin", "ticker"],
        }
    })
}

async fn get_settings(State(state): State<Arc<EmergencyState>>) -> Response {
    state.logger.info("emergency: retrieving settings");
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<EmergencyState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    state.logger.info("emergency: updating settings");
    let settings = match EmergencySettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => {
            state.logger.warn(&format!("emergency: failed to parse settings: {}", error));
            return fail(StatusCode::BAD_REQUEST, error.to_string());
        }
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        state.logger.error(&format!("emergency: failed to save settings: {}", error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info(&format!(
        "emergency: settings updated - alert_display={:?}, location='{}'",
        settings.alert_display, settings.location
    ));
    Json(view(&settings)).into_response()
}

/// Simulate an alert so the settings page can show what an active alert would
/// do, without needing a live NOAA watch.
async fn test_alerts(State(state): State<Arc<EmergencyState>>) -> Response {
    state.logger.info("emergency: testing alert display");
    let settings = state.settings.lock().await.clone();
    let (kind, detail): (&str, String) = match settings.alert_display {
        AlertDisplay::Off => (
            "off",
            "Alerts are off — no overlay will appear during programming.".to_string(),
        ),
        AlertDisplay::CutIn => (
            "cutin",
            format!(
                "Alerts screen every {} minutes for {} seconds; show audio ducks under the attention/end tones.",
                settings.cutin_interval_minutes,
                settings.cutin_duration_seconds
            ),
        ),
        AlertDisplay::Ticker => (
            "ticker",
            "Scrolling alert text overlays the bottom of the current program.".to_string(),
        ),
    };
    state.logger.info(&format!("emergency: test alert - mode={}, detail={}", kind, detail));
    Json(json!({
        "result": {
            "ok": settings.alert_display != AlertDisplay::Off,
            "display": kind,
            "detail": detail,
        }
    }))
    .into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// What the overlay would do right now, the way v1.0.0's WeatherAlertOverlay
/// Service did it: the mode, the active weather alerts for the location, and
/// the cut-in schedule. v1 pulled alerts from NOAA; Open-Meteo's alerts feed
/// covers the US and EU here.
async fn current_alerts(State(state): State<Arc<EmergencyState>>) -> Response {
    state.logger.info("emergency: fetching current alerts");
    let settings = state.settings.lock().await.clone();
    let mode = match settings.alert_display {
        AlertDisplay::Off => "off",
        AlertDisplay::CutIn => "cutin",
        AlertDisplay::Ticker => "ticker",
    };
    let next_cut_in_seconds = if mode == "cutin" {
        settings.cutin_interval_minutes.saturating_mul(60)
    } else {
        0
    };
    if settings.location.trim().is_empty() {
        state.logger.warn("emergency: cannot fetch alerts - location is not set");
        return Json(json!({
            "ok": false,
            "mode": mode,
            "error": "Set a location to fetch alerts.",
            "alerts": [],
            "next_cut_in_seconds": next_cut_in_seconds,
            "cutin_duration_seconds": settings.cutin_duration_seconds,
        }))
        .into_response();
    }

    state.logger.info(&format!("emergency: geocoding location '{}'", settings.location));
    let place = match geocode(&state.http, &state.logger, &settings.location).await {
        Ok(place) => {
            state.logger.info(&format!(
                "emergency: geocoded to '{}' ({}, {})",
                place.name, place.latitude, place.longitude
            ));
            place
        }
        Err(error) => {
            state.logger.error(&format!("emergency: geocoding failed: {}", error));
            return Json(json!({
                "ok": false,
                "mode": mode,
                "error": error,
                "alerts": [],
                "next_cut_in_seconds": next_cut_in_seconds,
                "cutin_duration_seconds": settings.cutin_duration_seconds,
            }))
            .into_response();
        }
    };

    // The source maps to units, not a different endpoint (Open-Meteo serves
    // the US and worldwide).
    let _ = settings.source;
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=temperature_2m,weather_code,wind_speed_10m\
         &alerts=true&temperature_unit=fahrenheit&wind_speed_unit=mph&timezone=auto",
        place.latitude, place.longitude
    );
    state.logger.info(&format!("emergency: fetching weather data from {}", url));
    let response = match state.http.get(&url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            state.logger.error(&format!("emergency: weather service returned status {}", response.status()));
            return Json(json!({
                "ok": false,
                "mode": mode,
                "error": format!("the weather service answered {}", response.status()),
                "alerts": [],
                "next_cut_in_seconds": next_cut_in_seconds,
                "cutin_duration_seconds": settings.cutin_duration_seconds,
            }))
            .into_response();
        }
        Err(error) => {
            state.logger.error(&format!("emergency: failed to reach weather service: {}", error));
            return Json(json!({
                "ok": false,
                "mode": mode,
                "error": "could not reach the weather service",
                "alerts": [],
                "next_cut_in_seconds": next_cut_in_seconds,
                "cutin_duration_seconds": settings.cutin_duration_seconds,
            }))
            .into_response();
        }
    };
    let Ok(body) = response.json::<serde_json::Value>().await else {
        state.logger.error("emergency: failed to parse weather service response");
        return Json(json!({
            "ok": false,
            "mode": mode,
            "error": "the weather service reply was unreadable",
            "alerts": [],
            "next_cut_in_seconds": next_cut_in_seconds,
            "cutin_duration_seconds": settings.cutin_duration_seconds,
        }))
        .into_response();
    };

    let alerts: Vec<serde_json::Value> = body
        .pointer("/alerts/alert")
        .and_then(serde_json::Value::as_array)
        .map(|alerts| {
            alerts
                .iter()
                .map(|alert| {
                    json!({
                        "event": alert.get("event"),
                        "headline": alert.get("headline"),
                        "description": alert.get("description"),
                        "severity": alert.get("severity"),
                        "urgency": alert.get("urgency"),
                        "starts_at": alert.get("start"),
                        "ends_at": alert.get("end"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let current = body.get("current").cloned();

    state.logger.info(&format!(
        "emergency: fetched {} alerts for location '{}'",
        alerts.len(),
        place.name
    ));

    Json(json!({
        "ok": true,
        "mode": mode,
        "location": { "name": place.name, "latitude": place.latitude, "longitude": place.longitude },
        "alerts": alerts,
        "count": alerts.len(),
        "next_cut_in_seconds": next_cut_in_seconds,
        "cutin_duration_seconds": settings.cutin_duration_seconds,
        "current": current,
    }))
    .into_response()
}

/// Resolve a location query to coordinates via Open-Meteo's geocoder (or
/// `latitude,longitude` directly).
async fn geocode(http: &reqwest::Client, logger: &channelflow_plugin_api::plugin::PluginLogger, query: &str) -> Result<GeoPlace, String> {
    let query = query.trim();
    logger.info(&format!("emergency: geocoding query '{}'", query));
    
    if let Some((latitude, longitude)) = parse_coordinates(query) {
        logger.info(&format!("emergency: parsed coordinates directly: ({}, {})", latitude, longitude));
        return Ok(GeoPlace {
            name: query.to_string(),
            latitude,
            longitude,
        });
    }
    
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        query.replace(' ', "+")
    );
    logger.info(&format!("emergency: calling geocoding API: {}", url));
    
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|error| {
            logger.error(&format!("emergency: failed to reach geocoding service: {}", error));
            "could not reach the location geocoder".to_string()
        })?;
    
    if !response.status().is_success() {
        let status = response.status();
        logger.error(&format!("emergency: geocoding service returned status {}", status));
        return Err(format!("the location geocoder answered {}", status));
    }
    
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| {
            logger.error(&format!("emergency: failed to parse geocoding response: {}", error));
            "the location geocoder reply was unreadable".to_string()
        })?;
    
    let Some(result) = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .and_then(|results| results.first())
    else {
        logger.warn(&format!("emergency: geocoding returned no results for '{}'", query));
        return Err(format!("could not find {query:?}"));
    };
    
    let place = GeoPlace {
        name: result
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(query)
            .to_string(),
        latitude: result.get("latitude").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
        longitude: result.get("longitude").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
    };
    
    logger.info(&format!(
        "emergency: geocoding resolved '{}' to ({}, {})",
        place.name, place.latitude, place.longitude
    ));
    
    Ok(place)
}

struct GeoPlace {
    name: String,
    latitude: f64,
    longitude: f64,
}

fn parse_coordinates(query: &str) -> Option<(f64, f64)> {
    let (latitude, longitude) = query.split_once(',')?;
    Some((
        latitude.trim().parse::<f64>().ok()?,
        longitude.trim().parse::<f64>().ok()?,
    ))
}