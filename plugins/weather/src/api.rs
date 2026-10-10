//! The API this plugin mounts. The base nests it under
//! `/api/plugins/com.channelflow.weather`, so the Weather page's calls to
//! `GET /`, `PUT /`, and `POST /test` resolve there.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use crate::settings::{ALL_SCREENS, WeatherSettings, SETTINGS_KEY};
use crate::WeatherState;

pub fn router(state: Arc<WeatherState>) -> Router {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/test", get(test_forecast))
        .with_state(state)
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
    let settings = state.settings.lock().await.clone();
    Json(view(&settings)).into_response()
}

async fn put_settings(
    State(state): State<Arc<WeatherState>>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let settings = match WeatherSettings::parse(&value) {
        Ok(settings) => settings,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let stored = serde_json::to_value(&settings).unwrap_or(value);
    if let Err(error) = state.storage.set(SETTINGS_KEY, &stored).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *state.settings.lock().await = settings.clone();
    state.logger.info("weather settings updated");
    Json(view(&settings)).into_response()
}

/// Resolve the saved location and fetch the current conditions, so the page
/// can prove the weather source works. Open-Meteo answers geocoding and the
/// forecast; NOAA is used downstream when the source forces it.
async fn test_forecast(State(state): State<Arc<WeatherState>>) -> Response {
    let settings = state.settings.lock().await.clone();
    if settings.default_location.trim().is_empty() {
        return Json(json!({
            "result": { "ok": false, "code": "no_location", "detail": "Set a location first." }
        }))
        .into_response();
    }
    let geocode_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        urlencoding(&settings.default_location)
    );
    let geocode = match state.http.get(&geocode_url).send().await {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) => body,
            Err(error) => {
                return Json(json!({
                    "result": { "ok": false, "code": "bad_url", "detail": format!("Geocoder reply unreadable: {error}") }
                }))
                .into_response();
            }
        },
        Ok(response) => {
            return Json(json!({
                "result": { "ok": false, "code": "bad_url", "detail": format!("Geocoder answered {}", response.status()) }
            }))
            .into_response();
        }
        Err(_) => {
            return Json(json!({
                "result": { "ok": false, "code": "unreachable", "detail": "Could not reach the weather geocoder" }
            }))
            .into_response();
        }
    };
    let Some(result) = geocode.get("results").and_then(Value::as_array).and_then(|r| r.first()) else {
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
                    Json(json!({ "result": { "ok": true, "code": "ok", "detail": detail } })).into_response()
                }
                Err(error) => fail(
                    StatusCode::BAD_GATEWAY,
                    format!("forecast reply unreadable: {error}"),
                ),
            }
        }
        Ok(response) => fail(
            StatusCode::BAD_GATEWAY,
            format!("forecast answered {}", response.status()),
        ),
        Err(_) => fail(
            StatusCode::BAD_GATEWAY,
            "could not reach the weather forecast API".to_string(),
        ),
    }
}

fn urlencoding(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join("+")
}

fn fail(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}