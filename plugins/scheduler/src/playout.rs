//! The playout writer: a generated day becomes ErsatzTV-next's
//! `{start}_{finish}.json` plus the channel's guide programmes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use channelflow_plugin_api::core::CoreData;
use channelflow_plugin_api::plugin::PluginLogger;
use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
use serde::Serialize;
use serde_json::{json, Value};

use crate::build::GeneratedDay;

/// The playout schema this writer targets.
pub const SCHEMA: &str = "https://ersatztv.org/playout/version/0.0.5";

/// A stand-in that fills a block's leftover time (the off-air slate).
const SLATE: &str = "color=c=0x101010:s=1280x720:r=30";

/// One placed item: the window it occupies and where it came from.
struct Segment {
    start: DateTime<FixedOffset>,
    finish: DateTime<FixedOffset>,
    match_key: String,
    title: String,
    why: String,
}

/// The local datetime `ms` milliseconds after local midnight of `date`.
fn at(date: &str, ms: i64) -> Option<DateTime<FixedOffset>> {
    let day = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let naive = day.and_hms_opt(0, 0, 0)?;
    let local = Local.from_local_datetime(&naive).single()?;
    Some((local + chrono::Duration::milliseconds(ms)).fixed_offset())
}

/// Lay each block's items into their windows from their runtimes, in order, so
/// code — never the model — decides start and finish.
fn segments(day: &GeneratedDay) -> Vec<Segment> {
    let mut out = Vec::new();
    for block in &day.blocks {
        let (Some(start), Some(finish)) = (at(&day.date, block.start_ms), at(&day.date, block.finish_ms))
        else {
            continue;
        };
        let mut cursor = start;
        for item in &block.items {
            let remaining = (finish - cursor).num_milliseconds();
            if remaining <= 0 {
                break;
            }
            let duration = item
                .runtime_ms
                .unwrap_or(remaining)
                .clamp(1, remaining);
            let item_finish = cursor + chrono::Duration::milliseconds(duration);
            out.push(Segment {
                start: cursor,
                finish: item_finish,
                match_key: item.match_key.clone(),
                title: item.title.clone(),
                why: item.why.clone(),
            });
            cursor = item_finish;
        }
        // Anything the items did not fill is a slate so the window is covered.
        if cursor < finish {
            out.push(Segment {
                start: cursor,
                finish,
                match_key: String::new(),
                title: "Off air".to_string(),
                why: "leftover".to_string(),
            });
        }
    }
    out
}

/// The playout document and the file name next looks for.
pub fn build_document(day: &GeneratedDay, paths: &HashMap<String, String>) -> (String, Value) {
    let placed = segments(day);
    let mut items = Vec::new();
    for (index, segment) in placed.iter().enumerate() {
        let source = match paths.get(&segment.match_key) {
            Some(path) if !segment.match_key.is_empty() => {
                json!({ "source_type": "local", "path": path })
            }
            _ => json!({ "source_type": "lavfi", "params": SLATE }),
        };
        items.push(json!({
            "id": format!("{}-{}", day.date, index),
            "start": segment.start.to_rfc3339(),
            "finish": segment.finish.to_rfc3339(),
            "source": source,
        }));
    }
    let start = placed.first().map(|segment| segment.start);
    let finish = placed.last().map(|segment| segment.finish);
    let name = match (start, finish) {
        (Some(start), Some(finish)) => format!("{}_{}.json", compact(start), compact(finish)),
        _ => format!("{}.json", day.date),
    };
    (name, json!({ "version": SCHEMA, "items": items }))
}

/// The guide programmes for a day: one per placed segment.
pub fn guide_programs(day: &GeneratedDay) -> Vec<Value> {
    segments(day)
        .into_iter()
        .map(|segment| {
            json!({
                "id": format!("{}-{}", day.date, segment.match_key),
                "start": segment.start.to_rfc3339(),
                "finish": segment.finish.to_rfc3339(),
                "title": segment.title,
                "description": segment.why,
            })
        })
        .collect()
}

/// The compact ISO 8601 form next's `parse_playout_filename` reads, e.g.
/// `20260413T000000.000000000-0500`.
fn compact(dt: DateTime<FixedOffset>) -> String {
    dt.format("%Y%m%dT%H%M%S%.9f%z").to_string()
}

/// What one write produced.
#[derive(Debug, Clone, Serialize)]
pub struct WriteReport {
    pub path: String,
    pub items: usize,
    /// How many items had no playable file and fell back to the slate.
    pub missing: usize,
}

/// Write a day's playout document into `dir` and set the channel's guide.
pub async fn write(
    core: &Arc<dyn CoreData>,
    day: &GeneratedDay,
    dir: &Path,
    logger: &PluginLogger,
) -> Result<WriteReport, String> {
    logger.info(&format!(
        "scheduler: writing playout for {} {} to {:?}",
        day.channel_id, day.date, dir
    ));
    
    let mut paths: HashMap<String, String> = HashMap::new();
    for block in &day.blocks {
        for item in &block.items {
            if item.match_key.is_empty() || paths.contains_key(&item.match_key) {
                continue;
            }
            if let Ok(Some(file)) = core.file_for(&item.match_key).await {
                paths.insert(item.match_key.clone(), file.path);
            } else {
                logger.warn(&format!(
                    "scheduler: no file found for match_key {}",
                    item.match_key
                ));
            }
        }
    }

    let (name, document) = build_document(day, &paths);
    let missing = segments(day)
        .iter()
        .filter(|segment| !segment.match_key.is_empty() && !paths.contains_key(&segment.match_key))
        .count();
    
    if missing > 0 {
        logger.warn(&format!(
            "scheduler: {} items missing files, will use slate",
            missing
        ));
    }
    
    std::fs::create_dir_all(dir).map_err(|error| {
        logger.error(&format!("scheduler: failed to create playout directory {:?}: {}", dir, error));
        error.to_string()
    })?;
    
    let path = dir.join(&name);
    let text = serde_json::to_string_pretty(&document).map_err(|error| {
        logger.error(&format!("scheduler: failed to serialize playout document: {}", error));
        error.to_string()
    })?;
    
    std::fs::write(&path, text).map_err(|error| {
        logger.error(&format!("scheduler: failed to write playout file {:?}: {}", path, error));
        error.to_string()
    })?;
    
    logger.info(&format!("scheduler: playout written to {:?}", path));

    set_guide(core, day, logger).await?;
    set_lineup(core, day, logger).await?;

    Ok(WriteReport {
        path: path.to_string_lossy().to_string(),
        items: document["items"].as_array().map(Vec::len).unwrap_or(0),
        missing,
    })
}

/// Set the channel's guide from a generated day. This is always called when
/// building the horizon, regardless of whether playout files are written.
pub async fn set_guide(
    core: &Arc<dyn CoreData>,
    day: &GeneratedDay,
    logger: &PluginLogger,
) -> Result<(), String> {
    logger.info(&format!(
        "scheduler: setting guide for {} {}",
        day.channel_id, day.date
    ));
    
    let programs = guide_programs(day);
    logger.info(&format!(
        "scheduler: guide contains {} programs",
        programs.len()
    ));
    
    core.set_channel_guide(&day.channel_id, programs)
        .await
        .map_err(|error| {
            logger.error(&format!(
                "scheduler: failed to set guide for {}: {}",
                day.channel_id, error
            ));
            error.to_string()
        })?;
    
    logger.info(&format!("scheduler: guide set successfully for {}", day.channel_id));
    Ok(())
}

/// Convert the generated blocks into lineup slots for the Lineups page.
/// Each block becomes a slot with its items as candidates.
pub fn blocks_to_lineup_slots(day: &GeneratedDay) -> Vec<Value> {
    day.blocks
        .iter()
        .map(|block| {
            let candidates: Vec<Value> = block
                .items
                .iter()
                .map(|item| {
                    json!({
                        "kind": "item",
                        "match_key": item.match_key,
                        "title": item.title,
                        "weight": 1,
                    })
                })
                .collect();
            json!({
                "slot_index": block.slot_index,
                "span_slots": block.span_slots,
                "is_rerun": false,
                "candidates": candidates,
            })
        })
        .collect()
}

/// Set the channel's lineup from a generated day. This populates the Lineups
/// page with the Scheduler's output so users can view and edit it.
pub async fn set_lineup(
    core: &Arc<dyn CoreData>,
    day: &GeneratedDay,
    logger: &PluginLogger,
) -> Result<(), String> {
    logger.info(&format!(
        "scheduler: setting lineup for {} {}",
        day.channel_id, day.date
    ));
    
    let slots = blocks_to_lineup_slots(day);
    logger.info(&format!(
        "scheduler: lineup contains {} slots",
        slots.len()
    ));
    
    core.set_channel_lineup(&day.channel_id, slots)
        .await
        .map_err(|error| {
            logger.error(&format!(
                "scheduler: failed to set lineup for {}: {}",
                day.channel_id, error
            ));
            error.to_string()
        })?;
    
    logger.info(&format!("scheduler: lineup set successfully for {}", day.channel_id));
    Ok(())
}

/// Where a channel's playout files go: `playout_dir` (or `<config>/playout`),
/// then the channel id.
pub fn channel_dir(config_root: &Path, configured: &str, channel: &str) -> PathBuf {
    let base = if configured.trim().is_empty() {
        config_root.join("playout")
    } else {
        PathBuf::from(configured.trim())
    };
    base.join(channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{AiredBlock, AiredItem};

    fn day() -> GeneratedDay {
        GeneratedDay {
            channel_id: "c1".into(),
            date: "2026-10-13".into(),
            base_hash: "b".into(),
            input_hash: "i".into(),
            blocks: vec![AiredBlock {
                slot_index: 0,
                span_slots: 4,
                start_ms: 6 * 60 * 60 * 1000,
                finish_ms: 8 * 60 * 60 * 1000,
                filled_ms: 90 * 60 * 1000,
                items: vec![AiredItem {
                    match_key: "movie:x".into(),
                    title: "X".into(),
                    runtime_ms: Some(90 * 60 * 1000),
                    why: "test".into(),
                }],
            }],
            degraded: false,
            provider: None,
            created_at: "now".into(),
            validation: vec![],
            prompt: None,
        }
    }

    #[test]
    fn a_day_becomes_a_covered_playout_document() {
        let mut paths = HashMap::new();
        paths.insert("movie:x".to_string(), "/media/x.mkv".to_string());
        let (name, document) = build_document(&day(), &paths);
        // The file name is the compact ISO start_finish pair.
        assert!(name.ends_with(".json"), "{name}");
        assert!(name.contains('_'), "{name}");
        assert_eq!(document["version"], SCHEMA);
        let items = document["items"].as_array().expect("items");
        // The 90-minute film plus a 30-minute slate fill the two-hour block.
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["source"]["source_type"], "local");
        assert_eq!(items[0]["source"]["path"], "/media/x.mkv");
        assert_eq!(items[1]["source"]["source_type"], "lavfi");
        // Items are contiguous and ordered.
        assert_eq!(items[0]["finish"], items[1]["start"]);
    }

    #[test]
    fn a_missing_file_falls_back_to_the_slate() {
        let (_name, document) = build_document(&day(), &HashMap::new());
        let items = document["items"].as_array().expect("items");
        assert!(items.iter().all(|item| item["source"]["source_type"] == "lavfi"));
    }

    #[test]
    fn the_guide_has_a_programme_per_segment() {
        let programs = guide_programs(&day());
        assert_eq!(programs.len(), 2);
        assert_eq!(programs[0]["title"], "X");
        assert!(programs[0]["start"].as_str().unwrap().contains('T'));
    }
}
