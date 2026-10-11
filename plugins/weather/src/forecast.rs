//! Live weather, the way the v1.0.0 Weather plugin fetched it: resolve the
//! location query to coordinates, then pull an Open-Meteo forecast snapshot
//! with current conditions and the daily outlook. NOAA was v1's preferred US
//! backend with Open-Meteo as the fallback; Open-Meteo covers both here, so
//! the whole forecast comes from that one call.

use serde_json::{json, Value};

use crate::settings::{Source, Units};

/// A resolved place to fetch weather for.
pub struct GeoPlace {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// Resolve a location query — `latitude,longitude`, a ZIP, a city, or any
/// place Open-Meteo's geocoder knows — to coordinates.
pub async fn geocode(http: &reqwest::Client, query: &str) -> Result<GeoPlace, String> {
    let query = query.trim();
    if let Some((lat, lon)) = parse_coordinates(query) {
        return Ok(GeoPlace {
            name: query.to_string(),
            latitude: lat,
            longitude: lon,
        });
    }
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        query.replace(' ', "+")
    );
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|_| "could not reach the weather geocoder".to_string())?;
    if !response.status().is_success() {
        return Err(format!("the weather geocoder answered {}", response.status()));
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|_| "the weather geocoder reply was unreadable".to_string())?;
    let Some(result) = body
        .get("results")
        .and_then(Value::as_array)
        .and_then(|results| results.first())
    else {
        return Err(format!("could not find {query:?}"));
    };
    Ok(GeoPlace {
        name: result
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(query)
            .to_string(),
        latitude: result.get("latitude").and_then(Value::as_f64).unwrap_or(0.0),
        longitude: result.get("longitude").and_then(Value::as_f64).unwrap_or(0.0),
    })
}

/// Fetch the current conditions and daily forecast for a place.
pub async fn snapshot(
    http: &reqwest::Client,
    place: &GeoPlace,
    source: Source,
    units: Units,
) -> Result<serde_json::Value, String> {
    let (temperature_unit, wind_speed_unit, wind_label, temp_symbol) = match units {
        Units::Us => ("fahrenheit", "mph", "mph", "°F"),
        Units::Si => ("celsius", "kmh", "km/h", "°C"),
    };
    // v1 chose NOAA for the US and Open-Meteo worldwide; Open-Meteo covers
    // both, so the source only shapes the units label here.
    let _ = source;
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m,wind_gusts_10m,wind_direction_10m,dew_point_2m,surface_pressure,visibility\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &temperature_unit={temperature_unit}&wind_speed_unit={wind_speed_unit}&timezone=auto",
        place.latitude, place.longitude
    );
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|_| "could not reach the weather forecast API".to_string())?;
    if !response.status().is_success() {
        return Err(format!("the weather forecast answered {}", response.status()));
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|_| "the weather forecast reply was unreadable".to_string())?;

    let current = body.get("current").cloned().unwrap_or_else(|| json!({}));
    let current_code = current.get("weather_code").and_then(Value::as_i64).unwrap_or(0);
    let (current_condition, current_icon) = condition(current_code);
    let hours_utc = body
        .get("utc_offset_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        / 3600;

    let daily = body.get("daily").cloned().unwrap_or_else(|| json!({}));
    let days: Vec<Value> = daily
        .get("time")
        .and_then(Value::as_array)
        .map(|times| {
            times
                .iter()
                .enumerate()
                .map(|(index, time)| {
                    let code = daily
                        .get("weather_code")
                        .and_then(Value::as_array)
                        .and_then(|codes| codes.get(index))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let (day_condition, day_icon) = condition(code);
                    json!({
                        "date": time.as_str().unwrap_or(""),
                        "code": code,
                        "condition": day_condition,
                        "icon": day_icon,
                        "max": daily.get("temperature_2m_max").and_then(Value::as_array).and_then(|arr| arr.get(index)).and_then(Value::as_f64),
                        "min": daily.get("temperature_2m_min").and_then(Value::as_array).and_then(|arr| arr.get(index)).and_then(Value::as_f64),
                        "precipitation_prob": daily.get("precipitation_probability_max").and_then(Value::as_array).and_then(|arr| arr.get(index)).and_then(Value::as_i64),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "location": { "query": place.name.clone(), "name": place.name.clone(), "latitude": place.latitude, "longitude": place.longitude },
        "units": { "temperature": temp_symbol, "wind": wind_label },
        "utc_offset_seconds": body.get("utc_offset_seconds").and_then(Value::as_i64).unwrap_or(0),
        "current": {
            "temperature": current.get("temperature_2m").and_then(Value::as_f64),
            "feels_like": current.get("apparent_temperature").and_then(Value::as_f64),
            "humidity": current.get("relative_humidity_2m").and_then(Value::as_i64),
            "wind_speed": current.get("wind_speed_10m").and_then(Value::as_f64),
            "wind_gusts": current.get("wind_gusts_10m").and_then(Value::as_f64),
            "wind_direction": current.get("wind_direction_10m").and_then(Value::as_i64),
            "dew_point": current.get("dew_point_2m").and_then(Value::as_f64),
            "pressure": current.get("surface_pressure").and_then(Value::as_f64),
            "visibility": current.get("visibility").and_then(Value::as_f64),
            "code": current_code,
            "condition": current_condition,
            "icon": current_icon,
            "time": current.get("time").and_then(Value::as_str),
        },
        "daily": days,
        "hours_utc": hours_utc,
    }))
}

/// `latitude,longitude` queries the geocoder would not know.
fn parse_coordinates(query: &str) -> Option<(f64, f64)> {
    let (lat, lon) = query.split_once(',')?;
    if lat.trim() == "0" || lon.trim() == "0" {
        return None;
    }
    let lat: f64 = lat.trim().parse().ok()?;
    let lon: f64 = lon.trim().parse().ok()?;
    Some((lat, lon))
}

/// The WMO weather code -> a friendly condition and an icon name, mirroring
/// the v1 weather icon map.
pub fn condition(code: i64) -> (&'static str, &'static str) {
    match code {
        0 => ("Clear sky", "sun"),
        1 | 2 => ("Partly cloudy", "cloud-sun"),
        3 => ("Overcast", "cloud"),
        45 | 48 => ("Fog", "fog"),
        51 | 53 | 55 | 56 | 57 => ("Drizzle", "drizzle"),
        61 | 63 | 65 | 66 | 67 => ("Rain", "rain"),
        71 | 73 | 75 | 77 => ("Snow", "snow"),
        80 | 81 | 82 => ("Rain showers", "showers"),
        85 | 86 => ("Snow showers", "snow"),
        95 => ("Thunderstorm", "thunder"),
        96 | 99 => ("Thunderstorm with hail", "thunder"),
        _ => ("Weather", "cloud"),
    }
}