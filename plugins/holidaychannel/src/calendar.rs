//! The Holiday Channel's seasonal calendar and content matching, ported from
//! v1.0.0's `HolidayChannelCalendar`.
//!
//! Each holiday has an observance date (fixed, Easter Sunday, or the US
//! Thanksgiving), a seasonal window around it, keywords to match against a
//! catalog item's title/overview (+ tags/genres when present), and a period
//! logo. The channel shows the nearest active holiday, or the next one if the
//! date falls outside every window.

use chrono::{Datelike, NaiveDate};

/// How a holiday's observance date is derived for a given year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateKind {
    Fixed,
    EasterSunday,
    UsThanksgiving,
}

/// One holiday: how it is observed, when its season runs, what matches it.
#[derive(Debug, Clone)]
pub struct Holiday {
    pub id: &'static str,
    pub name: &'static str,
    pub date_kind: DateKind,
    /// Fixed observance month (1-12) / day (1-31).
    pub observance_month: u32,
    pub observance_day: u32,
    /// Season starts up to this many days before the observance (capped at 30).
    pub days_before: i64,
    /// Season continues this many days after the observance.
    pub days_after: i64,
    /// Whether the window may bleed into the month before the observance
    /// (Halloween's October season).
    pub bleed_into_prior_month: bool,
    /// Logo for this holiday, relative to the plugin web root.
    pub logo: &'static str,
    /// An early-season logo change: the logo to use from the start of the
    /// observance's month through `early_logo_until_day`.
    pub early_logo: Option<(&'static str, u32)>,
    /// The words a media title/overview/tags/genres may carry.
    pub keywords: &'static [&'static str],
    /// Premiering in one of these months also matches (v2's catalog lacks
    /// premiere dates, so this is informational — the page matching focuses
    /// on the keywords).
    pub season_months: &'static [u32],
}

/// The largest seasonal lead-in any holiday gets.
pub const MAX_DAYS_BEFORE: i64 = 30;

pub const HOLIDAYS: &[Holiday] = &[
    Holiday {
        id: "valentines",
        name: "Valentine's Day",
        date_kind: DateKind::Fixed,
        observance_month: 2,
        observance_day: 14,
        days_before: 25,
        days_after: 0,
        bleed_into_prior_month: false,
        logo: "logos/valentines.png",
        early_logo: None,
        keywords: &["valentine", "valentines", "sweetheart", "cupid"],
        season_months: &[2],
    },
    Holiday {
        id: "easter",
        name: "Easter",
        date_kind: DateKind::EasterSunday,
        observance_month: 1, // unused for Easter
        observance_day: 1,
        days_before: 30,
        days_after: 0,
        bleed_into_prior_month: false,
        logo: "logos/easter.png",
        early_logo: None,
        keywords: &["easter", "bunny", "rabbit", "egg hunt", "resurrection", "paschal"],
        season_months: &[3, 4],
    },
    Holiday {
        id: "independence",
        name: "Independence Day",
        date_kind: DateKind::Fixed,
        observance_month: 7,
        observance_day: 4,
        days_before: 9,
        days_after: 0,
        bleed_into_prior_month: false,
        logo: "logos/independence.png",
        early_logo: None,
        keywords: &[
            "independence day",
            "july 4",
            "july fourth",
            "fourth of july",
            "4th of july",
            "fireworks",
            "patriotic",
        ],
        season_months: &[6, 7],
    },
    Holiday {
        id: "halloween",
        name: "Halloween",
        date_kind: DateKind::Fixed,
        observance_month: 10,
        observance_day: 31,
        days_before: 30,
        days_after: 0,
        bleed_into_prior_month: true,
        logo: "logos/halloween.png",
        // Early October shows the "October" art until the 15th.
        early_logo: Some(("logos/October.png", 15)),
        keywords: &[
            "halloween",
            "spooky",
            "ghost",
            "witch",
            "pumpkin",
            "haunted",
            "october",
            "trick or treat",
            "horror",
        ],
        season_months: &[10],
    },
    Holiday {
        id: "thanksgiving",
        name: "Thanksgiving",
        date_kind: DateKind::UsThanksgiving,
        observance_month: 1, // unused
        observance_day: 1,
        days_before: 30,
        days_after: 0,
        bleed_into_prior_month: false,
        logo: "logos/thanksgiving.png",
        early_logo: None,
        keywords: &["thanksgiving", "turkey", "pilgrim", "harvest feast"],
        season_months: &[11],
    },
    Holiday {
        id: "christmas",
        name: "Christmas",
        date_kind: DateKind::Fixed,
        observance_month: 12,
        observance_day: 25,
        days_before: 24,
        days_after: 0,
        bleed_into_prior_month: false,
        logo: "logos/christmas.png",
        early_logo: None,
        keywords: &[
            "christmas",
            "xmas",
            "santa",
            "nutcracker",
            "grinch",
            "snowman",
            "krampus",
            "noel",
            "nativity",
        ],
        season_months: &[11, 12],
    },
];

/// The date a holiday is observed in `year`.
pub fn observance_date(holiday: &Holiday, year: i32) -> NaiveDate {
    match holiday.date_kind {
        DateKind::Fixed => NaiveDate::from_ymd_opt(
            year,
            holiday.observance_month,
            holiday.observance_day,
        )
        .expect("valid fixed holiday date"),
        DateKind::EasterSunday => easter_sunday(year),
        DateKind::UsThanksgiving => us_thanksgiving(year),
    }
}

/// The inclusive seasonal window `(start, end)` around an observance.
pub fn window(
    holiday: &Holiday,
    observance: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    let mut start = observance - chrono::Duration::days(
        holiday.days_before.min(MAX_DAYS_BEFORE),
    );
    let end = observance + chrono::Duration::days(holiday.days_after);
    if holiday.bleed_into_prior_month {
        let prior_month_start = month_start(observance) - chrono::Duration::days(31);
        if start > prior_month_start {
            start = prior_month_start;
        }
    }
    (start, end)
}

/// The active holiday for `date`, or `None` outside every window.
pub fn active_holiday(date: NaiveDate) -> Option<&'static Holiday> {
    let mut best: Option<(&Holiday, i64)> = None;
    for holiday in HOLIDAYS {
        for year in [date.year() - 1, date.year(), date.year() + 1] {
            let observance = observance_date(holiday, year);
            let (start, end) = window(holiday, observance);
            if date < start || date > end {
                continue;
            }
            let distance = (date - observance).num_days().abs();
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((holiday, distance));
            }
        }
    }
    best.map(|(holiday, _)| holiday)
}

/// The active holiday or, outside every window, the nearest upcoming one that
/// falls within `max_days_before` days.
pub fn active_or_upcoming(date: NaiveDate, max_days_before: i64) -> Option<&'static Holiday> {
    if let Some(active) = active_holiday(date) {
        return Some(active);
    }
    let mut best: Option<(&Holiday, i64)> = None;
    for holiday in HOLIDAYS {
        for year in [date.year(), date.year() + 1] {
            let observance = observance_date(holiday, year);
            if observance < date {
                continue;
            }
            let distance = (observance - date).num_days();
            // Negative distance would mean the observance already passed.
            if distance < 0 || distance > max_days_before {
                continue;
            }
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((holiday, distance));
            }
        }
    }
    best.map(|(holiday, _)| holiday)
}

/// The logo the channel shows for `date`, relative to the plugin web root.
/// Off-season borrowed v1's airplane graphic.
pub fn logo_for(holiday: Option<&Holiday>, date: NaiveDate) -> &'static str {
    let Some(holiday) = holiday else {
        return "logos/off-season.png";
    };
    if let Some((path, until_day)) = holiday.early_logo {
        let observance = observance_date(holiday, date.year());
        if date.month() == observance.month() && date.day() <= until_day {
            return path;
        }
    }
    holiday.logo
}

/// Whether a catalog item's text (title + overview, plus tags/genres if the
/// item carries them) matches the holiday.
pub fn matches(item: &serde_json::Value, holiday: &Holiday) -> bool {
    let mut blob = String::new();
    for key in ["title", "overview"] {
        if let Some(text) = item.get(key).and_then(serde_json::Value::as_str) {
            blob.push(' ');
            blob.push_str(text);
        }
    }
    if let Some(tags) = item.get("tags").and_then(serde_json::Value::as_array) {
        for tag in tags {
            if let Some(text) = tag.as_str() {
                blob.push(' ');
                blob.push_str(text);
            }
        }
    }
    if let Some(genres) = item.get("genres").and_then(serde_json::Value::as_array) {
        for genre in genres {
            if let Some(text) = genre.as_str() {
                blob.push(' ');
                blob.push_str(text);
            }
        }
    }
    let lower = blob.to_lowercase();
    holiday
        .keywords
        .iter()
        .any(|keyword| lower.contains(&keyword.to_lowercase()))
}

/// The first day of `date`'s month.
fn month_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("valid month start")
}

/// The Sunday Easter falls on (Gauss's algorithm).
fn easter_sunday(year: i32) -> NaiveDate {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + (2 * e) + (2 * i) - h - k) % 7;
    let m = (a + (11 * h) + (22 * l)) / 451;
    let month = (h + l - (7 * m) + 114) / 31;
    let day = ((h + l - (7 * m) + 114) % 31) + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("valid easter date")
}

/// The fourth Thursday of November.
fn us_thanksgiving(year: i32) -> NaiveDate {
    let mut date = NaiveDate::from_ymd_opt(year, 11, 1).expect("valid november first");
    while date.weekday() != chrono::Weekday::Thu {
        date += chrono::Duration::days(1);
    }
    date + chrono::Duration::days(21)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_holiday_tracks_the_winter_season() {
        // 2026-12-20 is inside the Christmas window (24 days before Christmas).
        let date = NaiveDate::from_ymd_opt(2026, 12, 20).unwrap();
        assert_eq!(active_holiday(date).unwrap().id, "christmas");
    }

    #[test]
    fn easter_and_thanksgiving_are_computed() {
        let easter = observance_date(
            HOLIDAYS.iter().find(|h| h.id == "easter").unwrap(),
            2026,
        );
        assert_eq!(easter, NaiveDate::from_ymd_opt(2026, 4, 5).unwrap());
        let thanksgiving = observance_date(
            HOLIDAYS.iter().find(|h| h.id == "thanksgiving").unwrap(),
            2026,
        );
        // 2026: first Thursday is Nov 5; 4th Thursday is Nov 26.
        assert_eq!(thanksgiving, NaiveDate::from_ymd_opt(2026, 11, 26).unwrap());
    }

    #[test]
    fn halloween_early_october_uses_the_october_logo() {
        let halloween = HOLIDAYS.iter().find(|h| h.id == "halloween").unwrap();
        let early = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(logo_for(Some(halloween), early), "logos/October.png");
        let late = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        assert_eq!(logo_for(Some(halloween), late), "logos/halloween.png");
        let off = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
        assert_eq!(logo_for(None, off), "logos/off-season.png");
    }

    #[test]
    fn keywords_match_with_case_insensitivity() {
        let halloween = HOLIDAYS.iter().find(|h| h.id == "halloween").unwrap();
        let item = serde_json::json!({
            "match_key": "movie:halloweentown",
            "title": "Halloweentown",
            "overview": "A witch's daughter explores a spooky town.",
        });
        assert!(matches(&item, halloween));
        let item = serde_json::json!({
            "match_key": "movie:casablanca",
            "title": "Casablanca",
            "overview": "A nightclub owner in wartime Morocco.",
        });
        assert!(!matches(&item, halloween));
    }
}