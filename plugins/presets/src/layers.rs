//! The Scheduler layers each built-in preset applies to its channel.
//!
//! A preset creates a channel shell; these documents give the Scheduler
//! something to build from. They match the Scheduler's six-layer schema
//! (`HoursLayer`, the tagged `Rule` enum, `ItemsLayer`, free-text prompt) and
//! are applied as one bundle after the channel is created. Kept as JSON rather
//! than typed structs so this plugin does not depend on the Scheduler crate —
//! the Scheduler validates the bundle when it is applied.

use serde_json::{json, Value};

/// The layer bundle for a preset id, or `None` for a preset with no template
/// (the plugin-contributed ones, which bring their own behaviour).
pub fn layers_for(id: &str) -> Option<Value> {
    let bundle = match id {
        "channelflow-flashback" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "02:00", "to": "06:00" }],
                "dayparts": [
                    { "name": "Morning reruns", "from": "06:00", "to": "09:00", "kinds": ["series"] },
                    { "name": "Daytime", "from": "09:00", "to": "17:00", "kinds": [] },
                    { "name": "Primetime", "from": "19:00", "to": "23:00", "kinds": ["series", "movie"] }
                ]
            },
            "rules": [
                { "rule": "series_order", "order": "chronological" },
                { "rule": "no_repeat_within", "days": 30 },
                { "rule": "min_gap_between_same_series", "hours": 3 }
            ],
            "items": { "filter": { "kinds": ["series", "movie"], "genres": [], "tags": [], "year_min": 1970, "year_max": 2009, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "1970–2009 television and movies. Play series in first-episode-year order and lean on the era's sitcoms, dramas and made-for-TV films."
        }),
        "channelflow-retro" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "02:00", "to": "06:00" }],
                "dayparts": [{ "name": "Primetime", "from": "19:00", "to": "23:00", "kinds": ["series", "movie"] }]
            },
            "rules": [
                { "rule": "series_order", "order": "chronological" },
                { "rule": "no_repeat_within", "days": 45 }
            ],
            "items": { "filter": { "kinds": ["series", "movie"], "genres": [], "tags": [], "year_min": 1910, "year_max": 1969, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "1910–1969 black-and-white era television and films. Series play in first-episode-year order."
        }),
        "channelflow-open-swim" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "01:00", "to": "06:00" }],
                "dayparts": [
                    { "name": "Saturday morning cartoons", "from": "06:00", "to": "11:00", "kinds": ["series"] },
                    { "name": "Afternoon", "from": "11:00", "to": "17:00", "kinds": ["series"] },
                    { "name": "Evening", "from": "17:00", "to": "21:00", "kinds": [] }
                ]
            },
            "rules": [
                { "rule": "rating_ceiling", "rating": "TV-PG" },
                { "rule": "no_repeat_within", "days": 14 }
            ],
            "items": { "filter": { "kinds": ["series", "movie"], "genres": ["Animation", "Family", "Kids"], "tags": [], "year_min": null, "year_max": null, "rating_max": "TV-PG" }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Nick, Disney, Fox Kids and Cartoon Network style kids TV and movies. Anything from any year, nothing above TV-PG."
        }),
        "channelflow-reality" => json!({
            "hours": {
                "day_start": "07:00",
                "off_air": [{ "from": "02:00", "to": "07:00" }],
                "dayparts": [{ "name": "Marathon", "from": "12:00", "to": "18:00", "kinds": ["series"] }]
            },
            "rules": [
                { "rule": "max_per_day", "kind": "series", "n": 10 },
                { "rule": "no_repeat_within", "days": 7 }
            ],
            "items": { "filter": { "kinds": ["series"], "genres": ["Reality"], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Reality TV themed shows and movies; stack same-series episodes into afternoon marathons."
        }),
        "channelflow-past-tense-news" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [],
                "dayparts": [
                    { "name": "Morning bulletin", "from": "06:00", "to": "09:00", "kinds": ["news"] },
                    { "name": "Evening bulletin", "from": "18:00", "to": "20:00", "kinds": ["news"] }
                ]
            },
            "rules": [{ "rule": "block_align", "kinds": ["news"] }],
            "items": { "filter": { "kinds": ["news"], "genres": [], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Old news events from the local library, played as if they are breaking right now."
        }),
        "channelflow-crime" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "02:00", "to": "06:00" }],
                "dayparts": [{ "name": "Primetime case files", "from": "19:00", "to": "23:00", "kinds": ["series", "movie"] }]
            },
            "rules": [{ "rule": "no_repeat_within", "days": 21 }],
            "items": { "filter": { "kinds": ["series", "movie"], "genres": ["Crime", "Mystery", "Thriller"], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Crime and cop themed TV and movies, matched by genre or plot."
        }),
        "channelflow-comedy" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "03:00", "to": "06:00" }],
                "dayparts": [{ "name": "Slappy's Toon Takeover", "from": "17:00", "to": "20:00", "kinds": ["series"] }]
            },
            "rules": [{ "rule": "no_repeat_within", "days": 14 }],
            "items": { "filter": { "kinds": ["series", "movie"], "genres": ["Comedy"], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Fox network clone: comedy TV and movies, with Friday 5–8pm Slappy's Toon Takeover."
        }),
        "channelflow-game-shows" => json!({
            "hours": { "day_start": "07:00", "off_air": [{ "from": "01:00", "to": "07:00" }], "dayparts": [] },
            "rules": [
                { "rule": "max_per_day", "kind": "series", "n": 12 },
                { "rule": "no_repeat_within", "days": 10 }
            ],
            "items": { "filter": { "kinds": ["series"], "genres": [], "tags": ["Game Show", "Quiz"], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Game shows all day."
        }),
        "channelflow-education" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "02:00", "to": "06:00" }],
                "dayparts": [{ "name": "Class in session", "from": "09:00", "to": "15:00", "kinds": ["series"] }]
            },
            "rules": [{ "rule": "no_repeat_within", "days": 30 }],
            "items": { "filter": { "kinds": ["series", "movie"], "genres": ["Documentary"], "tags": ["Educational"], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Educational TV shows and movies."
        }),
        "channelflow-youtube" => json!({
            "hours": { "day_start": "00:00", "off_air": [], "dayparts": [] },
            "rules": [
                { "rule": "series_order", "order": "shuffle" },
                { "rule": "no_repeat_within", "days": 5 }
            ],
            "items": { "filter": { "kinds": ["series"], "genres": [], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Content from the Jellyfin TV library curated the way a YouTube feed plays — short, shuffled, endless."
        }),
        "channelflow-creature" => json!({
            "hours": {
                "day_start": "12:00",
                "off_air": [{ "from": "02:00", "to": "12:00" }],
                "dayparts": [{ "name": "Double Feature", "from": "20:00", "to": "00:00", "kinds": ["movie"] }]
            },
            "rules": [
                { "rule": "max_per_day", "kind": "movie", "n": 6 },
                { "rule": "no_repeat_within", "days": 30 }
            ],
            "items": { "filter": { "kinds": ["movie", "series"], "genres": ["Horror", "Science Fiction"], "tags": ["Monster", "Creature"], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Creature and monster movies and TV, matched by genre, plot or tags; pair them into late-night double features."
        }),
        "channelflow-hero" => json!({
            "hours": {
                "day_start": "06:00",
                "off_air": [{ "from": "02:00", "to": "06:00" }],
                "dayparts": [{ "name": "Primetime", "from": "19:00", "to": "23:00", "kinds": ["movie"] }]
            },
            "rules": [
                { "rule": "max_per_day", "kind": "movie", "n": 5 },
                { "rule": "no_repeat_within", "days": 21 }
            ],
            "items": { "filter": { "kinds": ["movie"], "genres": ["Action", "Adventure"], "tags": ["Superhero"], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Anyone who saves or protects people — heroes, rescuers and champions."
        }),
        "channelflow-funny" => json!({
            "hours": { "day_start": "10:00", "off_air": [{ "from": "02:00", "to": "10:00" }], "dayparts": [] },
            "rules": [{ "rule": "max_per_day", "kind": "movie", "n": 6 }],
            "items": { "filter": { "kinds": ["movie"], "genres": ["Comedy"], "tags": ["Stand-Up"], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Stand-up comedy specials and comedy movies/shows."
        }),
        "channelflow-parody" => json!({
            "hours": { "day_start": "00:00", "off_air": [], "dayparts": [] },
            "rules": [{ "rule": "no_repeat_within", "days": 3 }],
            "items": { "filter": { "kinds": ["musicvideo"], "genres": [], "tags": ["Parody"], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Parody music videos, wall to wall."
        }),
        "channelflow-rap" => json!({
            "hours": { "day_start": "00:00", "off_air": [], "dayparts": [] },
            "rules": [{ "rule": "no_repeat_within", "days": 3 }],
            "items": { "filter": { "kinds": ["musicvideo"], "genres": ["Hip Hop", "Rap"], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "Rap and hip hop music videos."
        }),
        "channelflow-music-video" => json!({
            "hours": { "day_start": "00:00", "off_air": [], "dayparts": [] },
            "rules": [{ "rule": "no_repeat_within", "days": 3 }],
            "items": { "filter": { "kinds": ["musicvideo"], "genres": [], "tags": [], "year_min": null, "year_max": null, "rating_max": null }, "list_ids": [], "match_keys": [], "pins": [] },
            "prompt": "All other music videos."
        }),
        _ => return None,
    };
    Some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_preset_has_a_bundle() {
        for preset in crate::presets::PRESETS {
            let bundle = layers_for(preset.id).unwrap_or_else(|| panic!("{} has no bundle", preset.id));
            assert!(bundle["hours"].is_object(), "{}: hours", preset.id);
            assert!(bundle["rules"].is_array(), "{}: rules", preset.id);
            assert!(bundle["items"]["filter"]["kinds"].is_array(), "{}: kinds", preset.id);
            assert!(bundle["prompt"].is_string(), "{}: prompt", preset.id);
        }
    }

    #[test]
    fn a_contributed_preset_has_no_bundle() {
        assert!(layers_for("channelflow-holiday").is_none());
    }
}
