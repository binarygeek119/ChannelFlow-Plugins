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

/// The FlowWire logo, shown centered over black while the intro music plays.
const LOGO_PNG: &[u8] = include_bytes!("../web/logos/FlowWire.png");

/// The news-desktop assets: a scrolling world map behind everything, the top
/// headline bar, the bottom story bar, and a mute overlay across the screen.
const MAP_PNG: &[u8] = include_bytes!("../web/media/map.png");
const HEADLINE_BAR_PNG: &[u8] = include_bytes!("../web/media/headline_bar.png");
const STORY_BAR_PNG: &[u8] = include_bytes!("../web/media/story_bar.png");
const MAP_MUTE_PNG: &[u8] = include_bytes!("../web/media/map_mute.png");

/// The FlowWire intro/outro music, from v1.0.0's FlowWire_News audio.
const INTRO_MP3: &[u8] = include_bytes!("../web/audio/FlowWire_News/FlowWire-intro.mp3");
const OUTRO_MP3: &[u8] = include_bytes!("../web/audio/FlowWire_News/FlowWire-outro.mp3");

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: u32 = 30;
const INTRO_SECONDS: f64 = 3.0;
const OUTRO_SECONDS: f64 = 3.0;
const SECONDS_PER_STORY: f64 = 6.0;
const MAX_TRACKED_STORIES: usize = 2000;
/// The anchor's spoken open, after the intro music finishes.
const ANCHOR_INTRO_SECONDS: f64 = 4.0;

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

    match render_bulletin(&settings, &state.http, &encoded, &work_dir, &staging).await {
        Ok(()) => {}
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
    }
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
/// Reads Google TTS speech for the anchor open, each story, and the outro
/// when TTS is enabled, and mixes it into the bed at each segment's window.
async fn render_bulletin(
    settings: &NewsSettings,
    http: &reqwest::Client,
    stories: &[Value],
    work_dir: &Path,
    staging: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(work_dir).map_err(|error| error.to_string())?;
    let logo = work_dir.join("logo.png");
    std::fs::write(&logo, LOGO_PNG).map_err(|error| error.to_string())?;
    let presenter = work_dir.join("presenter.jpg");
    std::fs::write(&presenter, PRESENTER_JPEG).map_err(|error| error.to_string())?;
    let map = work_dir.join("map.png");
    std::fs::write(&map, MAP_PNG).map_err(|error| error.to_string())?;
    let headline_bar = work_dir.join("headline_bar.png");
    std::fs::write(&headline_bar, HEADLINE_BAR_PNG).map_err(|error| error.to_string())?;
    let story_bar = work_dir.join("story_bar.png");
    std::fs::write(&story_bar, STORY_BAR_PNG).map_err(|error| error.to_string())?;
    let map_mute = work_dir.join("map_mute.png");
    std::fs::write(&map_mute, MAP_MUTE_PNG).map_err(|error| error.to_string())?;
    let intro_mp3 = work_dir.join("intro.mp3");
    std::fs::write(&intro_mp3, INTRO_MP3).map_err(|error| error.to_string())?;
    let outro_mp3 = work_dir.join("outro.mp3");
    std::fs::write(&outro_mp3, OUTRO_MP3).map_err(|error| error.to_string())?;

    let show_name = api::resolve_show_name(&settings.header);
    let intro_line = if settings.anchor_intro.trim().is_empty() {
        format!("I'm Catherine Wolfe. You're watching {show_name}.")
    } else {
        settings.anchor_intro.trim().to_string()
    };
    let outro_line = if settings.anchor_outro.trim().is_empty() {
        format!("I'm Catherine Wolfe. Stay with {show_name}.")
    } else {
        settings.anchor_outro.trim().to_string()
    };

    let intro_music_len = probe_seconds(&intro_mp3)
        .filter(|len| *len > 0.5)
        .unwrap_or(INTRO_SECONDS);
    let lead = intro_music_len.max(0.5);

    // Segment texts: the anchor open, each story's copy, the anchor outro.
    let mut texts: Vec<String> = vec![intro_line.clone()];
    for story in stories {
        texts.push(
            story
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
    }
    texts.push(outro_line.clone());

    // The anchor's copy is read aloud: Google TTS by default, or the AI engine
    // when it is chosen and an endpoint is set (AI falls back to Google if it
    // is not configured or fails).
    let ai_engine = matches!(settings.tts_engine, crate::settings::TtsEngine::Ai);
    let mut speech: Vec<Option<(PathBuf, f64)>> = vec![None; texts.len()];
    if settings.tts_enabled {
        for (index, text) in texts.iter().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let path = work_dir.join(format!("speech-{index}.mp3"));
            let audio_voice = if settings.ai_tts_voice.trim().is_empty() {
                &settings.tts_voice
            } else {
                &settings.ai_tts_voice
            };
            let result = if ai_engine && !settings.ai_tts_base_url.trim().is_empty() {
                crate::tts::synthesize_ai(
                    http,
                    text,
                    &settings.ai_tts_base_url,
                    &settings.ai_tts_api_key,
                    &settings.ai_tts_model,
                    audio_voice,
                    &path,
                )
                .await
            } else {
                crate::tts::synthesize(http, text, &settings.tts_voice, &path).await
            };
            if let Ok(count) = result {
                if count > 0 {
                    let duration = probe_seconds(&path).unwrap_or(0.0);
                    speech[index] = Some((path, duration));
                }
            }
        }
    }

    // The timeline; speech stretches each segment's window.
    let mut beats: Vec<Beat> = Vec::new();
    let mut starts: Vec<f64> = vec![lead];
    let mut t = lead;
    let intro_dur = speech[0]
        .as_ref()
        .map(|(_, duration)| duration.max(ANCHOR_INTRO_SECONDS))
        .unwrap_or(ANCHOR_INTRO_SECONDS);
    beats.push(Beat {
        start: t,
        end: t + intro_dur,
        title: show_name.clone(),
        body: intro_line,
    });
    starts.push(t + intro_dur);
    t += intro_dur;
    for (index, story) in stories.iter().enumerate() {
        let title = story.get("title").and_then(Value::as_str).unwrap_or("").to_string();
        let body = story.get("summary").and_then(Value::as_str).unwrap_or("").to_string();
        let speech_dur = speech.get(index + 1).and_then(|segment| segment.as_ref()).map(|(_, duration)| *duration).unwrap_or(0.0);
        let duration = story_seconds(&body).max(speech_dur).max(2.0);
        beats.push(Beat {
            start: t,
            end: t + duration,
            title,
            body,
        });
        starts.push(t + duration);
        t += duration;
    }
    let outro_dur = speech[texts.len() - 1]
        .as_ref()
        .map(|(_, duration)| duration.max(OUTRO_SECONDS))
        .unwrap_or(OUTRO_SECONDS);
    beats.push(Beat {
        start: t,
        end: t + outro_dur,
        title: show_name,
        body: outro_line,
    });
    starts.push(t + outro_dur);
    t += outro_dur;
    let show_total = t;

    let ass = build_ass(&beats);
    let ass_path = work_dir.join("news.ass");
    std::fs::write(&ass_path, ass).map_err(|error| error.to_string())?;

    // The outro music plays after the anchor outro; the video extends with it.
    let outro_len = probe_seconds(&outro_mp3)
        .filter(|len| *len > 0.5)
        .unwrap_or(OUTRO_SECONDS);
    let total = show_total + outro_len;
    let outro_ms = (show_total * 1000.0).round().max(0.0) as i64;

    // Audio: intro/outro music plus each speech segment at its window start.
    // Input indices: 7 = intro, 8 = outro, 9+ = speech.
    let mut audio_parts: Vec<String> = vec![
        format!("[7:a]atrim=0:{lead},asetpts=PTS-STARTPTS,loudnorm=I=-16:TP=-1.5:LRA=11,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[a0]"),
        format!("[8:a]atrim=0:{outro_len},asetpts=PTS-STARTPTS,loudnorm=I=-16:TP=-1.5:LRA=11,adelay={outro_ms}:all=1,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[a1]"),
    ];
    let mut labels: Vec<String> = vec!["a0".to_string(), "a1".to_string()];
    let mut audio_input = 9usize;
    for (index, segment) in speech.iter().enumerate() {
        let Some((_, duration)) = segment else {
            continue;
        };
        let label = format!("a{}", labels.len());
        let start_ms = (starts[index] * 1000.0).round().max(0.0) as i64;
        audio_parts.push(format!(
            "[{audio_input}:a]atrim=0:{duration},asetpts=PTS-STARTPTS,loudnorm=I=-16:TP=-1.5:LRA=11,adelay={start_ms}:all=1,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[{label}]"
        ));
        labels.push(label);
        audio_input += 1;
    }
    let mix = format!(
        "[{}]amix=inputs={}:duration=longest:normalize=0[aout]",
        labels.join("]["),
        labels.len()
    );
    let audio = format!("{};{mix}", audio_parts.join(";"));

    // Layers, bottom to top: the scrolling map behind everything, the anchor
    // centered, the headline bar over the top, the story bar over the bottom,
    // the captions, then the mute overlay across the whole screen.
    // While the intro music plays, only the FlowWire logo shows over black;
    // after the lead the desktop (map, anchor, bars, captions, mute) fades in.
    let filter = format!(
        "[0:v]trim=duration={total},setpts=PTS-STARTPTS[src];\
         [src]split[base][bg];\
         [1:v]scale=6480:1080[mp];\
         [bg][mp]overlay=x='-mod(t*150,6480)':y=0[m1];\
         [m1][mp]overlay=x='-mod(t*150,6480)+6480':y=0[m2];\
         [2:v]scale=760:760[p];\
         [m2][p]overlay=(W-w)/2:(H-h)/2[l1];\
         [3:v]scale=1920:1080[hb];\
         [l1][hb]overlay=0:0[l2];\
         [4:v]scale=1920:1080[sb];\
         [l2][sb]overlay=0:0[l3];\
         [5:v]scale=1920:1080[mm];\
         [l3][mm]overlay=0:0[desk];\
         [base][desk]overlay=0:0:enable='gte(t\\,{lead})'[v1];\
         [6:v]scale=560:-1[lg];\
         [v1][lg]overlay=(W-w)/2:(H-h)/2:enable='lt(t\\,{lead})'[v2];\
         [v2]ass='{ass}'[final];\
         {audio}",
        total = total,
        lead = lead,
        ass = escape_filter_path(&ass_path),
        audio = audio,
    );

    let mut args: Vec<String> = vec![
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
        map.to_string_lossy().to_string(),
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
        headline_bar.to_string_lossy().to_string(),
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        FPS.to_string(),
        "-t".into(),
        format!("{total}"),
        "-i".into(),
        story_bar.to_string_lossy().to_string(),
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        FPS.to_string(),
        "-t".into(),
        format!("{total}"),
        "-i".into(),
        map_mute.to_string_lossy().to_string(),
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        FPS.to_string(),
        "-t".into(),
        format!("{total}"),
        "-i".into(),
        logo.to_string_lossy().to_string(),
        "-i".into(),
        intro_mp3.to_string_lossy().to_string(),
        "-i".into(),
        outro_mp3.to_string_lossy().to_string(),
    ];
    for segment in speech.iter().flatten() {
        args.push("-i".into());
        args.push(segment.0.to_string_lossy().to_string());
    }
    args.extend([
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[final]".into(),
        "-map".into(),
        "[aout]".into(),
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
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "128k".into(),
        "-ac".into(),
        "2".into(),
        "-ar".into(),
        "48000".into(),
        "-movflags".into(),
        "+faststart".into(),
        // The staging name ends in `.partial`, which ffmpeg cannot use to pick
        // a container — state it.
        "-f".into(),
        "mp4".into(),
        staging.to_string_lossy().to_string(),
    ]);

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

/// The length of an audio file, if ffprobe is available.
fn probe_seconds(path: &Path) -> Option<f64> {
    let probe = {
        let bundled = PathBuf::from("/usr/local/bin/ffprobe");
        if bundled.exists() {
            bundled
        } else {
            PathBuf::from("ffprobe")
        }
    };
    let output = Command::new(probe)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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

/// Render the captions for the given beats: a Headline for the title and a
/// two-line Story event for the copy, in the top and bottom bars.
fn build_ass(beats: &[Beat]) -> String {
    let mut events = String::new();
    for beat in beats {
        if !beat.title.trim().is_empty() {
            let title = escape_ass(&beat.title);
            // Scroll headlines longer than 45 chars: animate right-to-left, loop if needed.
            if title.chars().count() > 45 {
                let duration = (beat.end - beat.start).max(4.0);
                // Speed: ~30 chars per second, minimum 8s scroll
                let scroll_ms = (title.chars().count() as f64 * 1000.0 / 30.0).max(8000.0) as i64;
                // \move(x1,y1,x2,y2,t1,t2) - start right edge, end left edge
                let move_tag = format!("{{\\move(1920,110,-800,110,0,{scroll_ms})}}");
                events.push_str(&format!(
                    "Dialogue: 0,{0},{1},Headline,,0,0,0,,{{\\b1}}{2}{3}{{\\b0}}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    move_tag,
                    title
                ));
            } else {
                events.push_str(&format!(
                    "Dialogue: 0,{},{},Headline,,0,0,0,,{{\\b1}}{}{{\\b0}}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    title
                ));
            }
        }
        let body = clean_text(&beat.body);
        if !body.is_empty() && !body.eq_ignore_ascii_case(beat.title.trim()) {
            let (line1, line2) = split_two_lines(&body);
            if line2.is_empty() {
                // Single line centered in the story bar
                events.push_str(&format!(
                    "Dialogue: 1,{},{},Story1,,0,0,0,,{}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    escape_ass(&line1)
                ));
            } else {
                // Two lines stacked
                events.push_str(&format!(
                    "Dialogue: 1,{},{},Story1,,0,0,0,,{}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    escape_ass(&line1)
                ));
                events.push_str(&format!(
                    "Dialogue: 2,{},{},Story2,,0,0,0,,{}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    escape_ass(&line2)
                ));
            }
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
    // Headline: top-centre, smaller font to fit more, positioned in the bar.
    // Story1/Story2: bottom-centre, tighter spacing for the two lines.
    ass.push_str("Style: Headline, Liberation Sans, 44, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 1, 0, 0, 0, 100, 100, 0, 0, 1, 3, 0, 8, 100, 100, 110, 1\n");
    ass.push_str("Style: Story1, Liberation Sans, 32, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 0, 0, 0, 0, 100, 100, 0, 0, 1, 2, 0, 2, 100, 100, 80, 1\n");
    ass.push_str("Style: Story2, Liberation Sans, 32, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 0, 0, 0, 0, 100, 100, 0, 0, 1, 2, 0, 2, 100, 100, 115, 1\n\n");
    ass.push_str("[Events]\n");
    ass.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    ass.push_str(&events);
    ass
}

/// A story's on-screen time grows with its copy, bounded so a long summary
/// cannot stall the show: 4–14 seconds.
fn story_seconds(body: &str) -> f64 {
    let chars = clean_text(body).chars().count();
    (3.0 + chars as f64 / 16.0).clamp(4.0, 14.0)
}


/// Split text into two balanced lines for the story bar, respecting word
/// boundaries. Returns (line1, line2) where line2 may be empty.
fn split_two_lines(text: &str) -> (String, String) {
    const MAX_PER_LINE: usize = 48;
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return (String::new(), String::new());
    }
    let mut line1 = String::new();
    let mut line2 = String::new();
    let mut on_line1 = true;
    for word in words {
        let target = if on_line1 { &mut line1 } else { &mut line2 };
        if target.is_empty() {
            target.push_str(word);
        } else if target.len() + 1 + word.len() <= MAX_PER_LINE {
            target.push(' ');
            target.push_str(word);
        } else if on_line1 && line2.is_empty() {
            // Switch to line 2
            on_line1 = false;
            line2.push_str(word);
        } else {
            // Line 2 full, append with ellipsis
            line2.push_str(" …");
            break;
        }
    }
    (line1, line2)
}

/// Greedy wrap of the copy into at most two lines (bottom bar), with an
/// ellipsis when a line overflows. Kept narrow because the story font is
/// large.
fn wrap_two(text: &str) -> String {
    const MAX: usize = 40;
    let mut words = text.split_whitespace();
    let mut first = String::new();
    while let Some(word) = words.next() {
        if !first.is_empty() && first.len() + word.len() + 1 > MAX {
            break;
        }
        if !first.is_empty() {
            first.push(' ');
        }
        first.push_str(word);
        if first.len() >= MAX {
            break;
        }
    }
    let mut second = String::new();
    for word in words {
        if !second.is_empty() && second.len() + word.len() + 1 > MAX {
            second.push_str(" …");
            break;
        }
        if !second.is_empty() {
            second.push(' ');
        }
        second.push_str(word);
    }
    if second.is_empty() {
        if first.len() > MAX {
            first.push_str(" …");
        }
        first
    } else {
        format!("{first}\\N{second}")
    }
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
    fn ass_puts_the_headline_in_the_bar_and_the_copy_as_two_lines() {
        let stories = vec![json!({ "title": "Headline One", "summary": "Spoken copy here." })];
        let beats = vec![
            Beat { start: 0.0, end: 3.0, title: "FlowWire News".into(), body: String::new() },
            Beat { start: 3.0, end: 7.0, title: "Headline One".into(), body: "Spoken copy here.".into() },
        ];
        let ass = build_ass(&beats);
        assert!(ass.contains("Headline One"));
        assert!(ass.contains("Spoken copy here."));
        assert!(ass.contains("Style: Story"));
        assert!(ass.contains("Headline One"));
        assert!(ass.contains("Spoken copy here."));
        assert!(!ass.contains("Crawl"), "the crawl is gone, two-line copy replaces it");
    }

    #[test]
    fn shorter_copy_gets_a_shorter_window() {
        let short = story_seconds("Short.");
        let long = story_seconds("This is a considerably longer summary with many more words in it.");
        assert!(long > short, "longer copy holds the screen longer");
        assert!(short >= 4.0 && long <= 14.0, "bounded so it cannot stall the show");
    }

    #[test]
    fn long_lines_wrap_onto_two_news_lines() {
        let text = "word ".repeat(40) + "overflow";
        let wrapped = wrap_two(&text);
        assert!(wrapped.contains("\\N"), "overflow wraps to a second line");
    }
}
