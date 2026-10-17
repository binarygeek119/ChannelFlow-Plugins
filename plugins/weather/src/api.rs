//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.weather`, so the Weather page's calls to
//! `GET /`, `PUT /`, and `POST /test` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::settings::{ALL_SCREENS, WeatherSettings, SETTINGS_KEY};
use crate::WeatherState;

pub fn router(state: Arc<WeatherState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/test", get(test_forecast))
        .route("/weather", get(weather_snapshot))
        .route("/guide", get(guide_status))
        .route("/guide/build", post(guide_build))
        .with_state(state)
}

/// The Weather page's guide status: the schedule and the last build.
async fn guide_status(State(state): State<Arc<WeatherState>>) -> Response {
    state.logger.info("weather: retrieving guide status");
    let settings = state.settings.lock().await.clone();
    let last = crate::guide::last_build_date(&state).await;
    
    state.logger.info(&format!(
        "weather: guide status - enabled={}, build_time={}, last_build={:?}",
        settings.guide_enabled, settings.guide_build_time, last
    ));
    
    Json(json!({
        "guide": {
            "enabled": settings.guide_enabled,
            "build_time": settings.guide_build_time,
            "last_build_date": last,
        }
    }))
    .into_response()
}

/// The Weather page's "Build guide now": force a build regardless of schedule.
async fn guide_build(State(state): State<Arc<WeatherState>>) -> Response {
    state.logger.info("weather: manual guide build requested");
    
    match crate::guide::build(&state, true).await {
        Ok(count) => {
            state.logger.info(&format!("weather: guide build successful - {} blocks created", count));
            crate::guide::record_build(&state).await;
            Json(json!({ "ok": true, "blocks": count })).into_response()
        }
        Err(error) => {
            state.logger.error(&format!("weather: guide build failed: {}", error));
            (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response()
        }
    }
}

fn view(settings: &WeatherSettings) -> serde_json::Value {
    json!({
        "settings": settings,
        "options": {
            "star_variants": ["ws4kp", "ws3kp"],
            "sources": ["auto", "us", "world"],
            "units": ["us", "si"],
            "screens": ALL_SCREENS,
        }
    })
}

async fn get_settings(State(state): State<Arc<WeatherState>>) -> Response {
    state.logger.info("weather: retrieving settings");
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<WeatherState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    state.logger.info("weather: updating settings");
    
    let settings = match WeatherSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => {
            state.logger.warn(&format!("weather: failed to parse settings: {}", error));
            return fail(StatusCode::BAD_REQUEST, error.to_string());
        }
    };
    
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        state.logger.error(&format!("weather: failed to save settings: {}", error));
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    
    state.logger.info(&format!(
        "weather: settings updated - location='{}', guide_enabled={}, source={:?}",
        settings.default_location, settings.guide_enabled, settings.source
    ));
    
    Json(view(&settings)).into_response()
}

/// Resolve the saved location and fetch the current conditions, so the page
/// can prove the weather source works. Open-Meteo answers geocoding and the
/// forecast; NOAA is used downstream when the source forces it.
async fn test_forecast(State(state): State<Arc<WeatherState>>) -> Response {
    state.logger.info("weather: testing forecast connection");
    
    let settings = state.settings.lock().await.clone();
    if settings.default_location.trim().is_empty() {
        state.logger.warn("weather: test forecast failed - no location set");
        return Json(json!({
            "result": { "ok": false, "code": "no_location", "detail": "Set a location first." }
        }))
        .into_response();
    }
    
    state.logger.info(&format!("weather: geocoding location '{}'", settings.default_location));
    let geocode_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        urlencoding(&settings.default_location)
    );
    let geocode = match state.http.get(&geocode_url).send().await {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) => {
                state.logger.info("weather: geocoding successful");
                body
            }
            Err(error) => {
                state.logger.error(&format!("weather: geocoding reply unreadable: {}", error));
                return Json(json!({
                    "result": { "ok": false, "code": "bad_url", "detail": format!("Geocoder reply unreadable: {error}") }
                }))
                .into_response();
            }
        },
        Ok(response) => {
            state.logger.warn(&format!("weather: geocoder answered {}", response.status()));
            return Json(json!({
                "result": { "ok": false, "code": "bad_url", "detail": format!("Geocoder answered {}", response.status()) }
            }))
            .into_response();
        }
        Err(error) => {
            state.logger.error(&format!("weather: could not reach geocoder: {}", error));
            return Json(json!({
                "result": { "ok": false, "code": "unreachable", "detail": "Could not reach the weather geocoder" }
            }))
            .into_response();
        }
    };
    
    let Some(result) = geocode.get("results").and_then(Value::as_array).and_then(|r| r.first()) else {
        state.logger.warn(&format!("weather: could not find location '{}'", settings.default_location));
        return Json(json!({
            "result": { "ok": false, "code": "no_location", "detail": format!("Could not find {:?}", settings.default_location) }
        }))
        .into_response();
    };
    
    let latitude = result.get("latitude").and_then(Value::as_f64).unwrap_or(0.0);
    let longitude = result.get("longitude").and_then(Value::as_f64).unwrap_or(0.0);
    let name = result
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("the location")
        .to_string();
    
    state.logger.info(&format!("weather: geocoded to '{}' ({}, {})", name, latitude, longitude));
    state.logger.info("weather: fetching forecast");

    let forecast_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}&current_weather=true"
    );
    match state.http.get(&forecast_url).send().await {
        Ok(response) if response.status().is_success() => {
            match response.json::<Value>().await {
                Ok(body) => {
                    let temperature = body["current_weather"]["temperature"].as_f64();
                    let detail = match temperature {
                        Some(temperature) => format!("{name}: {temperature}°C right now"),
                        None => format!("{name}: current conditions available"),
                    };
                    state.logger.info(&format!("weather: forecast test successful - {}", detail));
                    Json(json!({ "result": { "ok": true, "code": "ok", "detail": detail } })).into_response()
                }
                Err(error) => {
                    state.logger.error(&format!("weather: forecast reply unreadable: {}", error));
                    fail(
                        StatusCode::BAD_GATEWAY,
                        format!("forecast reply unreadable: {error}"),
                    )
                }
            }
        }
        Ok(response) => {
            state.logger.warn(&format!("weather: forecast answered {}", response.status()));
            fail(
                StatusCode::BAD_GATEWAY,
                format!("forecast answered {}", response.status()),
            )
        }
        Err(error) => {
            state.logger.error(&format!("weather: could not reach forecast API: {}", error));
            fail(
                StatusCode::BAD_GATEWAY,
                "could not reach the weather forecast API".to_string(),
            )
        }
    }
}

fn urlencoding(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join("+")
}

/// The live weather snapshot — current conditions and daily forecast for the
/// saved location, the way v1.0.0's WeatherDataClient served it. Fetches are
/// cached for a few minutes so the page doesn't hammer the weather API.
async fn weather_snapshot(State(state): State<Arc<WeatherState>>) -> Response {
    state.logger.info("weather: fetching weather snapshot");
    
    let settings = state.settings.lock().await.clone();
    if settings.default_location.trim().is_empty() {
        state.logger.warn("weather: snapshot failed - no location set");
        return Json(json!({
            "ok": false,
            "error": "Set a location first.",
        }))
        .into_response();
    }
    
    let key = format!(
        "{}|{:?}|{:?}",
        settings.default_location, settings.source, settings.units
    );
    
    {
        let cache = state.weather_cache.lock().await;
        if let Some((cached_key, taken_at, value)) = cache.as_ref() {
            if cached_key == &key && taken_at.elapsed().as_secs() < 5 * 60 {
                state.logger.info(&format!("weather: returning cached snapshot ({}s old)", taken_at.elapsed().as_secs()));
                return Json(json!({ "ok": true, "snapshot": value, "cached": true })).into_response();
            }
        }
    }
    
    state.logger.info(&format!("weather: geocoding location '{}'", settings.default_location));
    let place = match crate::forecast::geocode(&state.http, &settings.default_location).await {
        Ok(place) => {
            state.logger.info(&format!("weather: geocoded to '{}' ({}, {})", place.name, place.latitude, place.longitude));
            place
        }
        Err(error) => {
            state.logger.error(&format!("weather: geocoding failed: {}", error));
            return Json(json!({ "ok": false, "error": error })).into_response();
        }
    };
    
    state.logger.info("weather: fetching weather snapshot from API");
    let snapshot = match crate::forecast::snapshot(&state.http, &place, settings.source, settings.units).await {
        Ok(snapshot) => {
            state.logger.info("weather: snapshot fetched successfully");
            snapshot
        }
        Err(error) => {
            state.logger.error(&format!("weather: snapshot fetch failed: {}", error));
            return Json(json!({ "ok": false, "error": error, "location": json!({ "name": place.name }) }))
                .into_response();
        }
    };
    
    let response = Json(json!({ "ok": true, "snapshot": snapshot, "cached": false }));
    {
        let mut cache = state.weather_cache.lock().await;
        *cache = Some((key, std::time::Instant::now(), snapshot));
        state.logger.info("weather: snapshot cached for future requests");
    }
    response.into_response()
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}