//! The Holiday Channel plugin's API, mounted by the base under
//! `/api/plugins/com.channelflow.holidaychannel`:
//!   `/calendar` — which holiday is active (and the year's windows), for any
//!                 date;
//!   `/lineup`   — the active holiday's logo and the catalog items that match
//!                 it. The matching runs here (the v1 port); the page supplies
//!                 the catalog rows, which only the authenticated front end
//!                 can fetch.

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use channelflow_plugin_api::plugin::PluginLogger;
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::calendar;

#[derive(Clone)]
pub struct RoutesState {
    pub logger: PluginLogger,
}

pub fn router(logger: PluginLogger) -> Router {
    Router::new()
        .route("/calendar", get(calendar_json))
        .route("/lineup", post(lineup))
        .with_state(RoutesState { logger })
}

#[derive(Deserialize)]
struct CalendarQuery {
    /// `YYYY-MM-DD`; defaults to the server's local date.
    #[serde(default)]
    date: Option<String>,
}

fn parse_date(value: Option<&str>) -> NaiveDate {
    value
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .unwrap_or_else(|| chrono::Local::now().date_naive())
}

fn holiday_json(holiday: &'static calendar::Holiday, year: i32) -> Value {
    let observance = calendar::observance_date(holiday, year);
    let (start, end) = calendar::window(holiday, observance);
    json!({
        "id": holiday.id,
        "name": holiday.name,
        "logo": holiday.logo,
        "observed": observance.to_string(),
        "window_start": start.to_string(),
        "window_end": end.to_string(),
    })
}

/// The active (and upcoming) holiday plus every holiday's window, for the
/// current and following year.
async fn calendar_json(
    State(state): State<RoutesState>,
    Query(query): Query<CalendarQuery>,
) -> impl IntoResponse {
    let date = parse_date(query.date.as_deref());
    state.logger.info(&format!("holidaychannel: calendar requested for date {}", date));
    
    let active = calendar::active_holiday(date);
    let upcoming = calendar::active_or_upcoming(date, calendar::MAX_DAYS_BEFORE);
    
    if let Some(holiday) = active {
        state.logger.info(&format!("holidaychannel: active holiday for {} is '{}'", date, holiday.name));
    } else {
        state.logger.info(&format!("holidaychannel: no active holiday for {}", date));
    }
    
    let holidays: Vec<Value> = calendar::HOLIDAYS
        .iter()
        .map(|holiday| {
            json!({
                "id": holiday.id,
                "name": holiday.name,
                "logo": holiday.logo,
                "keywords": holiday.keywords,
                "season_months": holiday.season_months,
                "this_year": holiday_json(holiday, date.year()),
                "next_year": holiday_json(holiday, date.year() + 1),
            })
        })
        .collect();
    Json(json!({
        "date": date.to_string(),
        "active": active.map(|holiday| holiday_json(holiday, date.year())),
        "upcoming": upcoming.map(|holiday| holiday_json(holiday, date.year())),
        "logo": calendar::logo_for(active, date),
        "holidays": holidays,
    }))
}

#[derive(Deserialize)]
struct LineupBody {
    /// `YYYY-MM-DD`; defaults to the server's local date.
    #[serde(default)]
    date: Option<String>,
    /// The catalog rows to match (as `/api/media` returns them).
    #[serde(default)]
    items: Vec<Value>,
}

/// Match the supplied catalog rows against the holiday active on `date` and
/// return the lineup the channel would play, with the logo to show.
async fn lineup(
    State(state): State<RoutesState>,
    body: Json<LineupBody>,
) -> Response {
    let date = parse_date(body.date.as_deref());
    state.logger.info(&format!("holidaychannel: lineup requested for {} with {} items", date, body.items.len()));
    
    let Some(holiday) = calendar::active_holiday(date) else {
        state.logger.info(&format!("holidaychannel: no active holiday for {}, returning off-season lineup", date));
        return Json(json!({
            "date": date.to_string(),
            "holiday": Value::Null,
            "logo": "logos/off-season.png",
            "matches": [],
            "off_season": true,
            "off_season_media": "The Holiday Channel.mkv",
        }))
        .into_response();
    };
    
    state.logger.info(&format!("holidaychannel: active holiday for {} is '{}'", date, holiday.name));
    let observance = calendar::observance_date(holiday, date.year());
    let (start, end) = calendar::window(holiday, observance);
    let matches: Vec<Value> = body
        .items
        .iter()
        .filter(|item| calendar::matches(item, holiday))
        .map(|item| {
            json!({
                "match_key": item.get("match_key"),
                "kind": item.get("kind"),
                "title": item.get("title"),
                "year": item.get("year"),
                "overview": item.get("overview"),
                "poster_path": item.get("poster_path"),
            })
        })
        .collect();
    
    state.logger.info(&format!("holidaychannel: matched {} items for holiday '{}'", matches.len(), holiday.name));
    
    Json(json!({
        "date": date.to_string(),
        "holiday": {
            "id": holiday.id,
            "name": holiday.name,
            "logo": holiday.logo,
            "observed": observance.to_string(),
            "window_start": start.to_string(),
            "window_end": end.to_string(),
        },
        "logo": calendar::logo_for(Some(holiday), date),
        "matches": matches,
        "off_season": false,
    }))
    .into_response()
}