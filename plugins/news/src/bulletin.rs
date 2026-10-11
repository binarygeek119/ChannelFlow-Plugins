//! The News bulletin task: rebuild the FlowWire newscast as an mp4.
//!
//! 1.0.0's bulletin service rebuilt a short newscast video every few hours and
//! kept the newest one for the channel to play. This is the same shape on the
//! 2.0.0 engine: at each scheduled mark it fetches the feeds, counts the
//! stories it has not broadcast yet, and — unless this is the day's first
//! build — only re-encodes when at least `minimum_new_stories` are new. The
//! video itself is composed with ffmpeg overlays: a dark background, the
//! anchor's portrait, and an ASS caption layer with each headline and a
//! crawl of the spoken copy, exactly as 1.0.0's NewsAssBuilder laid them out.
//!
//! The output lands in `<config>/news`, the folder 1.0.0 used.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api;
use crate::settings::NewsSettings;
use crate::NewsState;

/// Where the plugin's bulletin ledger lives in its namespaced storage.
pub const LEDGER_KEY: &str = "bulletin-ledger";

/// The anchor's portrait, embedded so the renderer needs no external file.
const PRESENTER_JPEG: &[u8] = include_bytes!("../web/media/Catherine_Wolfe.jpg");

/// The FlowWire logo, shown as a centered bumper for the intro.
const LOGO_PNG: &[u8] = include_bytes!("../web/logos/FlowWire.png");

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: u32 = 30;
const INTRO_SECONDS: f64 = 3.0;
const OUTRO_SECONDS: f64 = 3.0;
const SECONDS_PER_STORY: f64 = 6.0;
const MAX_TRACKED_STORIES: usize = 2000;

/// The plugin's record of what it has broadcast, so a run can tell new
/// stories from ones already on air.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StoryLedger {
    #[serde(default)]
    pub used: Vec<String>,
    /// The local date (`YYYY-MM-DD`) of the last successful build, for the
    /// once-a-day guarantee.
    #[serde(default)]
    pub last_built_date: Option<String>,
    #[serde(default)]
    pub last_video_path: Option<String>,
    #[serde(default)]
    pub last_created: bool,
    #[serde(default)]
    pub last_skip_reason: Option<String>,
    #[serde(default)]
    pub last_new_count: usize,
    #[serde(default)]
    pub last_encoded_count: usize,
    /// Local timestamp of the last run, for status.
    #[serde(default)]
    pub last_run: Option<String>,
}

/// What one run produced.
#[derive(Debug, Clone, Serialize)]
pub struct BulletinOutcome {
    pub created: bool,
    pub skipped: bool,
    pub path: Option<String>,
    pub new_count: usize,
    pub encoded_count: usize,
    pub reason: Option<String>,
    pub ran_at: String,
    pub required: bool,
}

/// The key a story is tracked by — its headline, lowercased. Two runs that see
/// the same headline treat it as the same story.
pub fn story_key(story: &Value) -> String {
    story
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// The local date an instant falls on.
fn local_date<Tz: TimeZone>(now: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    now.format("%Y-%m-%d").to_string()
}

/// Whether the current local time is one of the configured bulletin marks.
pub fn at_a_mark(settings: &NewsSettings, now: chrono::DateTime<Local>) -> bool {
    use chrono::Timelike;
    settings.bulletin_hours().contains(&now.hour()) && now.minute() == 0
}

async fn load_ledger(state: &Arc<NewsState>) -> StoryLedger {
    match state.storage.get(LEDGER_KEY).await {
        Ok(Some(value)) => serde_json::from_value(value).unwrap_or_default(),
        _ => StoryLedger::default(),
    }
}

async fn save_ledger(state: &Arc<NewsState>, ledger: &StoryLedger) {
    if let Ok(value) = serde_json::to_value(ledger) {
        let _ = state.storage.set(LEDGER_KEY, &value).await;
    }
}

/// Run one bulletin: fetch, gate on new stories, render, and record.
///
/// `force` (a manual "build now") always re-encodes. Otherwise the day's first
/// scheduled build always runs; later builds only when at least
/// `minimum_new_stories` stories are new — otherwise the current video stands.
pub async fn run(
    state: &Arc<NewsState>,
    config_root: &Path,
    force: bool,
) -> BulletinOutcome {
    let settings = state.settings.lock().await.clone();
    let now = Local::now();
    let today = local_date(now);
    let mut ledger = load_ledger(state).await;
    let ran_at = now.to_rfc3339();

    if !settings.bulletin_enabled && !force {
        return finish_skip(state, ledger, ran_at, "Bulletin videos are turned off.".to_string(), 0, false).await;
    }

    let stories = api::fetch_enabled_stories(state, &settings).await;
    let used: std::collections::HashSet<&str> =
        ledger.used.iter().map(String::as_str).collect();
    let new: Vec<Value> = stories
        .iter()
        .filter(|story| {
            let key = story_key(story);
            !key.is_empty() && !used.contains(key.as_str())
        })
        .cloned()
        .collect();

    let first_today = ledger.last_built_date.as_deref() != Some(today.as_str());
    let required = force || first_today;
    let min = settings.minimum_new_stories.max(1) as usize;

    if !required && new.is_empty() {
        return finish_skip(state, ledger, ran_at, "No new stories.".to_string(), 0, false).await;
    }
    if !required && new.len() < min {
        let plural = if new.len() == 1 { "story" } else { "stories" };
        return finish_skip(
            state,
            ledger,
            ran_at,
            format!("Only {} new {plural} (minimum {min}).", new.len()),
            new.len(),
            false,
        )
        .await;
    }

    // Encode the newest stories, capped at the article count. The day's first
    // build re-encodes even with no new stories (so a video exists).
    let encoded: Vec<Value> = if new.is_empty() {
        stories.iter().take(settings.article_count.max(1) as usize).cloned().collect()
    } else {
        new.iter().take(settings.article_count.max(1) as usize).cloned().collect()
    };
    if encoded.is_empty() {
        return finish_skip(state, ledger, ran_at, "No stories to encode.".to_string(), 0, false).await;
    }

    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let folder = config_root.join("news");
    let work_dir = folder.join(format!("work-{stamp}"));
    let output = folder.join(format!("news-{stamp}.mp4"));
    let staging = output.with_extension("mp4.partial");

    let rendered = match render_bulletin(&settings, &encoded, &work_dir, &staging) {
        Ok(()) => true,
        Err(error) => {
            state
                .logger
                .warn(&format!("news bulletin render failed: {error}"));
            let _ = std::fs::remove_file(&staging);
            let _ = std::fs::remove_dir_all(&work_dir);
            return finish_skip(
                state,
                ledger,
                ran_at,
                format!("FFmpeg failed to create the news video. {error}"),
                new.len(),
                false,
            )
            .await;
        }
    };
    let _ = std::fs::remove_dir_all(&work_dir);

    if let Err(error) = std::fs::rename(&staging, &output) {
        let _ = std::fs::remove_file(&staging);
        state.logger.warn(&format!("could not publish the news video: {error}"));
        return finish_skip(
            state,
            ledger,
            ran_at,
            "Could not publish the news video.".to_string(),
            new.len(),
            false,
        )
        .await;
    }

    for story in &encoded {
        let key = story_key(story);
        if !key.is_empty() && !ledger.used.contains(&key) {
            ledger.used.push(key);
        }
    }
    if ledger.used.len() > MAX_TRACKED_STORIES {
        let drop = ledger.used.len() - MAX_TRACKED_STORIES;
        ledger.used.drain(0..drop);
    }
    ledger.last_built_date = Some(today.clone());
    ledger.last_video_path = Some(output.to_string_lossy().to_string());
    ledger.last_created = true;
    ledger.last_skip_reason = None;
    ledger.last_new_count = new.len();
    ledger.last_encoded_count = encoded.len();
    ledger.last_run = Some(ran_at.clone());
    save_ledger(state, &ledger).await;

    state.logger.info(&format!(
        "news bulletin created {} with {} of {} new stories",
        output.display(),
        encoded.len(),
        new.len()
    ));

    BulletinOutcome {
        created: true,
        skipped: false,
        path: Some(output.to_string_lossy().to_string()),
        new_count: new.len(),
        encoded_count: encoded.len(),
        reason: None,
        ran_at,
        required,
    }
}

async fn finish_skip(
    state: &Arc<NewsState>,
    mut ledger: StoryLedger,
    ran_at: String,
    reason: String,
    new_count: usize,
    required: bool,
) -> BulletinOutcome {
    ledger.last_created = false;
    ledger.last_skip_reason = Some(reason.clone());
    ledger.last_new_count = new_count;
    ledger.last_encoded_count = 0;
    ledger.last_run = Some(ran_at.clone());
    let path = ledger.last_video_path.clone();
    save_ledger(state, &ledger).await;
    if !required {
        state
            .logger
            .info(&format!("news bulletin skipped: {reason}"));
    }
    BulletinOutcome {
        created: false,
        skipped: true,
        path,
        new_count,
        encoded_count: 0,
        reason: Some(reason),
        ran_at,
        required,
    }
}

/// The status the page shows.
pub fn describe_ledger(ledger: &StoryLedger, settings: &NewsSettings) -> Value {
    serde_json::json!({
        "enabled": settings.bulletin_enabled,
        "minimum_new_stories": settings.minimum_new_stories,
        "times": settings.bulletin_times,
        "last_built_date": ledger.last_built_date,
        "last_created": ledger.last_created,
        "last_skip_reason": ledger.last_skip_reason,
        "last_video_path": ledger.last_video_path,
        "last_new_count": ledger.last_new_count,
        "last_encoded_count": ledger.last_encoded_count,
        "last_run": ledger.last_run,
        "used_stories": ledger.used.len(),
    })
}

/// Read the ledger for the API status route.
pub async fn ledger_snapshot(state: &Arc<NewsState>) -> StoryLedger {
    load_ledger(state).await
}

/// Compose the newscast mp4 with ffmpeg overlays, writing a staging file.
fn render_bulletin(
    settings: &NewsSettings,
    stories: &[Value],
    work_dir: &Path,
    staging: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(work_dir).map_err(|error| error.to_string())?;
    let presenter = work_dir.join("presenter.jpg");
    std::fs::write(&presenter, PRESENTER_JPEG).map_err(|error| error.to_string())?;
    let logo = work_dir.join("logo.png");
    std::fs::write(&logo, LOGO_PNG).map_err(|error| error.to_string())?;

    let show_name = api::resolve_show_name(&settings.header);
    let intro = if settings.anchor_intro.trim().is_empty() {
        format!("I'm Catherine Wolfe. You're watching {show_name}.")
    } else {
        settings.anchor_intro.trim().to_string()
    };
    let outro = if settings.anchor_outro.trim().is_empty() {
        format!("I'm Catherine Wolfe. Stay with {show_name}.")
    } else {
        settings.anchor_outro.trim().to_string()
    };

    let (ass, total) = build_ass(stories, &intro, &outro, &show_name);
    let ass_path = work_dir.join("news.ass");
    std::fs::write(&ass_path, ass).map_err(|error| error.to_string())?;

    // The FlowWire logo is the intro bumper (centered), then the anchor's
    // portrait centers for the rest of the show; the logo overlay runs after
    // the portrait's, so it sits on top during the intro window.
    let filter = format!(
        "[0:v]trim=duration={total},setpts=PTS-STARTPTS[bg];\
         [1:v]scale=520:-1[p];\
         [2:v]scale=460:-1[lg];\
         [bg][p]overlay=(W-w)/2:(H-h)/2:enable='gte(t\\,{intro})'[vp];\
         [vp][lg]overlay=(W-w)/2:(H-h)/2:enable='lt(t\\,{intro})'[vout];\
         [vout]ass='{ass}'[final]",
        total = total,
        intro = INTRO_SECONDS,
        ass = escape_filter_path(&ass_path)
    );

    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("color=c=0x101010:s={WIDTH}x{HEIGHT}:r={FPS}"),
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        FPS.to_string(),
        "-t".into(),
        format!("{total}"),
        "-i".into(),
        presenter.to_string_lossy().to_string(),
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        FPS.to_string(),
        "-t".into(),
        format!("{total}"),
        "-i".into(),
        logo.to_string_lossy().to_string(),
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[final]".into(),
        "-t".into(),
        format!("{total}"),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "21".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-g".into(),
        "30".into(),
        "-bf".into(),
        "0".into(),
        "-movflags".into(),
        "+faststart".into(),
        // The staging name ends in `.partial`, which ffmpeg cannot use to pick
        // a container — state it.
        "-f".into(),
        "mp4".into(),
        staging.to_string_lossy().to_string(),
    ];

    let output = Command::new(encoder_path())
        .args(&args)
        .output()
        .map_err(|error| format!("could not run ffmpeg: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" ");
        return Err(if tail.is_empty() {
            format!("ffmpeg exited {}", output.status)
        } else {
            tail
        });
    }
    if std::fs::metadata(staging).map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Err("ffmpeg produced an empty file".to_string());
    }
    Ok(())
}

/// Where ffmpeg lives: the ErsatzTV base image ships it in `/usr/local/bin`,
/// a dev box usually has it on `PATH`.
fn encoder_path() -> PathBuf {
    let bundled = PathBuf::from("/usr/local/bin/ffmpeg");
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from("ffmpeg")
    }
}

fn escape_filter_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

/// One on-screen beat: the headline shown and the spoken copy crawled under it.
struct Beat {
    start: f64,
    end: f64,
    title: String,
    body: String,
}

/// Build the ASS caption file and return it with the total show length.
/// Mirrors 1.0.0's NewsAssBuilder: a Headline at the top, a Crawl of the
/// spoken copy along the bottom, and the anchor's name throughout.
fn build_ass(stories: &[Value], intro: &str, outro: &str, show_name: &str) -> (String, f64) {
    let mut beats: Vec<Beat> = Vec::new();
    let mut t = 0.0f64;

    // Intro card.
    beats.push(Beat {
        start: t,
        end: t + INTRO_SECONDS,
        title: show_name.to_string(),
        body: intro.to_string(),
    });
    t += INTRO_SECONDS;

    for story in stories {
        let title = story
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let body = story
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        beats.push(Beat {
            start: t,
            end: t + SECONDS_PER_STORY,
            title,
            body,
        });
        t += SECONDS_PER_STORY;
    }

    beats.push(Beat {
        start: t,
        end: t + OUTRO_SECONDS,
        title: show_name.to_string(),
        body: outro.to_string(),
    });
    t += OUTRO_SECONDS;
    let total = t.max(1.0);

    let mut events = String::new();
    for beat in &beats {
        if !beat.title.trim().is_empty() {
            events.push_str(&format!(
                "Dialogue: 0,{},{},Headline,,0,0,0,,{{\\b1}}{}{{\\b0}}\n",
                ass_time(beat.start),
                ass_time(beat.end),
                escape_ass(&beat.title)
            ));
        }
        let body = clean_text(&beat.body);
        if !body.is_empty() && !body.eq_ignore_ascii_case(beat.title.trim()) {
            events.push_str(&format!(
                "Dialogue: 1,{},{},Crawl,,0,0,0,,{}\n",
                ass_time(beat.start),
                ass_time(beat.end),
                crawl(&body, beat.end - beat.start)
            ));
        }
    }

    let mut ass = String::new();
    ass.push_str("[Script Info]\n");
    ass.push_str("Title: FlowWire News\n");
    ass.push_str("ScriptType: v4.00+\n");
    ass.push_str("WrapStyle: 2\n");
    ass.push_str(&format!("PlayResX: {WIDTH}\n"));
    ass.push_str(&format!("PlayResY: {HEIGHT}\n\n"));
    ass.push_str("[V4+ Styles]\n");
    ass.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    ass.push_str("Style: Headline, Liberation Sans, 52, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 1, 0, 0, 0, 100, 100, 0, 0, 1, 3, 0, 8, 60, 60, 40, 1\n");
    ass.push_str("Style: Crawl, Liberation Sans, 64, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 0, 0, 0, 0, 100, 100, 0, 0, 1, 3, 0, 2, 40, 40, 60, 1\n");
    ass.push_str("Style: Presenter, Liberation Sans, 34, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 0, 0, 0, 0, 100, 100, 0, 0, 1, 2, 0, 9, 24, 24, 24, 1\n\n");
    ass.push_str("[Events]\n");
    ass.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    ass.push_str(&events);
    (ass, total)
}

/// The spoken copy as a bottom crawl that scrolls right-to-left.
fn crawl(text: &str, duration: f64) -> String {
    let line = escape_ass(text);
    let y = HEIGHT - 140;
    let x1 = WIDTH + 60;
    let text_width = (WIDTH / 2).max((line.chars().count() as u32) * 40);
    let seconds = duration.max(0.5);
    let move_ms = ((seconds - 0.25).max(0.4) * 1000.0).round() as i64;
    let x2 = -(text_width as i64);
    format!("{{\\q2\\move({x1},{y},{x2},{y},0,{move_ms})}}{line}")
}

fn ass_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    let total_cs = (seconds * 100.0).round() as i64;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

fn escape_ass(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('{', "(")
        .replace('}', ")")
        .replace('\r', "")
        .replace('\n', " ")
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_day_is_built_at_least_once() {
        // First run of the day is required even with no new stories; later
        // runs are not.
        let now: chrono::DateTime<Local> = Local.with_ymd_and_hms(2026, 10, 11, 6, 0, 0).unwrap();
        let settings = NewsSettings::default();
        assert!(at_a_mark(&settings, now), "06:00 is a default mark");
        assert!(settings.bulletin_hours().contains(&6));
        assert!(settings.bulletin_hours().contains(&22));
    }

    #[test]
    fn only_configured_hours_are_marks() {
        let settings = NewsSettings::default();
        let not_mark: chrono::DateTime<Local> = Local.with_ymd_and_hms(2026, 10, 11, 7, 0, 0).unwrap();
        assert!(!at_a_mark(&settings, not_mark), "07:00 is not a mark");
        let off_minute: chrono::DateTime<Local> = Local.with_ymd_and_hms(2026, 10, 11, 6, 30, 0).unwrap();
        assert!(!at_a_mark(&settings, off_minute), "only the top of the hour fires");
    }

    #[test]
    fn story_keys_are_the_lowercased_headline() {
        assert_eq!(story_key(&json!({ "title": "  Big News  " })), "big news");
        assert_eq!(story_key(&json!({})), "");
    }

    #[test]
    fn ass_lays_out_a_headline_and_a_crawl_per_story() {
        let stories = vec![json!({ "title": "Headline One", "summary": "Spoken copy." })];
        let (ass, total) = build_ass(&stories, "Intro.", "Outro.", "FlowWire News");
        assert!(ass.contains("Headline One"));
        assert!(ass.contains("Spoken copy."));
        assert!(ass.contains("Crawl"));
        // intro + 1 story + outro = 3 + 6 + 3
        assert!((total - 12.0).abs() < 0.001);
    }
}
