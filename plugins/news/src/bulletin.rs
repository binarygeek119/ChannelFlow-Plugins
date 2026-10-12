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

use chrono::{Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api;
use crate::rewrite;
use crate::settings::NewsSettings;
use crate::NewsState;
use channelflow_plugin_api::core::TtsProvider;
use channelflow_plugin_api::plugin::PluginLogger;

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

/// The scrolling world map is a panorama that loops forever: two copies are laid
/// end to end and slid left, and the scroll position wraps at one tile width so
/// the copy that has run off the left is immediately replaced on the right.
const MAP_SRC_W: u32 = 3884;
const MAP_SRC_H: u32 = 648;
/// The trailing columns of `map.png` fade to transparent; they are cropped off
/// so the end of one copy butts cleanly against the start of the next and no
/// translucent seam is left behind.
const MAP_OPAQUE_W: u32 = 3880;
/// The map lives only in the band between the headline bar and the story bar
/// (`MAP_TOP`..`MAP_TOP + MAP_BAND_H`): each copy is scaled to that band's
/// height rather than being stretched over the whole screen. It stays the
/// backmost layer, so the bars, the anchor and the captions sit over it.
const MAP_TOP: u32 = 221;
const MAP_BAND_H: u32 = 639;
/// The width every copy is scaled to. It is derived from the band height so the
/// panorama keeps its aspect ratio, and it doubles as the tile pitch so the
/// copies meet exactly. `MAP_SCROLL_SPEED` is how fast the map slides left, in
/// pixels per second.
const MAP_TILE_W: u32 = MAP_OPAQUE_W * MAP_BAND_H / MAP_SRC_H;
const MAP_SCROLL_SPEED: u32 = 150;
const _: () = assert!(MAP_OPAQUE_W <= MAP_SRC_W);

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
/// The logo eases up from black at the very start, over the first stretch of the
/// intro music.
const INTRO_FADE_SECONDS: f64 = 1.2;
/// After the anchor's close the desktop fades to black, the FlowWire logo fades
/// up over black while the outro music plays, and as the music ends the logo
/// fades back out to black over `OUTRO_FADE_SECONDS`; the video ends as that
/// fade completes.
const OUTRO_FADE_SECONDS: f64 = 1.2;
/// How long the anchor's close takes to fade the desktop (the presenter on the
/// map) to black before the outro logo fades up.
const OUTRO_DESKTOP_FADE_SECONDS: f64 = 1.0;
/// How long the FlowWire logo takes to fade up over black at the start of the
/// outro music.
const OUTRO_LOGO_IN_SECONDS: f64 = 0.8;
/// How long the anchor's screen takes to fade up from black once the intro
/// music (and logo) is done.
const DESKTOP_FADE_IN_SECONDS: f64 = 0.8;
/// How far the two story lines are raised off the bottom of the frame (the ASS
/// `MarginV`): line 1, then line 2 nearest the bottom. Both sit down in the
/// marked story areas.
const STORY_LINE1_MARGIN_V: i32 = 126;
const STORY_LINE2_MARGIN_V: i32 = 17;

/// The headline is a single line centred in the bright band of the headline bar
/// (rows 43–197 of the 0–221 bar), vertically centred here.
const HEADLINE_CENTER_Y: i32 = 120;
/// The width one headline line is grown to fill, using the same side margins
/// as the story bar. A headline too short to reach this at the maximum size is
/// centred instead of stretched.
const HEADLINE_TARGET_PX: f64 = 1700.0;
/// The smallest and largest headline sizes. The headline bar is much taller
/// than the story bar, so a headline can be far bigger than the copy; only once
/// it cannot fit at the floor does it crawl instead of shrinking further.
const HEADLINE_MIN_PX: f64 = 40.0;
const HEADLINE_MAX_PX: f64 = 96.0;

/// The window a story's picture is shown in: the bright box in the middle of
/// the map band, where the anchor's portrait sits. A picture is scaled to fit
/// here without changing its aspect ratio, drawn over the map, the mute overlay
/// and the anchor, and kept clear of the headline and story bars — its height is
/// the map band's, so the bars always stay on top of their own areas.
const IMAGE_BOX_X: u32 = 650;
const IMAGE_BOX_Y: u32 = 221;
const IMAGE_BOX_W: u32 = 650;
const IMAGE_BOX_H: u32 = 639;
/// How long a story's picture takes to fade up as the story opens and back out
/// as it closes, so it eases over the map instead of popping.
const IMAGE_FADE_SECONDS: f64 = 0.4;

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
    let mut encoded: Vec<Value> = if new.is_empty() {
        stories.iter().take(settings.article_count.max(1) as usize).cloned().collect()
    } else {
        new.iter().take(settings.article_count.max(1) as usize).cloned().collect()
    };
    if encoded.is_empty() {
        return finish_skip(state, ledger, ran_at, "No stories to encode.".to_string(), 0, false).await;
    }

    // The ledger tracks a story by its original headline, so remember the keys
    // before the AI rewrite changes the copy.
    let ledger_keys: Vec<String> = encoded.iter().map(story_key).collect();

    // With the option on, hand the copy to the AI writer so the bulletin reads
    // as one broadcast rather than a stack of articles.
    if settings.ai_rewrite {
        encoded = rewrite::rewrite_show(state, &settings, &encoded).await;
    }

    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let folder = config_root.join("news");
    let work_dir = folder.join(format!("work-{stamp}"));
    let output = folder.join(format!("news-{stamp}.mp4"));
    let staging = output.with_extension("mp4.partial");

    let ai_provider = state.core.tts_provider().await.ok().flatten();
    let (hour, minute) = (now.hour(), now.minute());
    match render_bulletin(
        &settings,
        &state.http,
        &encoded,
        &work_dir,
        &staging,
        ai_provider,
        hour,
        minute,
    )
    .await
    {
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

    // The new video is published, so the previous one has served its purpose:
    // drop it now and leave `<config>/news` holding only the latest newscast.
    remove_old_videos(&folder, &output, &state.logger);

    for key in &ledger_keys {
        if !key.is_empty() && !ledger.used.contains(key) {
            ledger.used.push(key.clone());
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

/// Delete every earlier bulletin video once `keep` has been published, so the
/// news folder keeps only the newest newscast. Only `news-*.mp4` files are
/// touched; the freshly written `keep` and anything unrelated are left alone.
fn remove_old_videos(folder: &Path, keep: &Path, logger: &PluginLogger) {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let is_bulletin = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("news-") && name.ends_with(".mp4"));
        if !is_bulletin {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => logger.info(&format!(
                "removed the previous news video {}",
                path.display()
            )),
            Err(error) => logger.warn(&format!(
                "could not remove the previous news video {}: {error}",
                path.display()
            )),
        }
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
/// Reads the anchor open, each story, and the outro aloud when TTS is enabled
/// (the AI page's provider, or Google TTS), and mixes the speech into the bed
/// at each segment's window.
async fn render_bulletin(
    settings: &NewsSettings,
    http: &reqwest::Client,
    stories: &[Value],
    work_dir: &Path,
    staging: &Path,
    ai_provider: Option<TtsProvider>,
    hour: u32,
    minute: u32,
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

    // Download each story's picture when the feed carries one. Anything that is
    // not really an image is dropped here rather than handed to ffmpeg, where a
    // bad input would sink the whole bulletin.
    let mut story_images: Vec<Option<PathBuf>> = vec![None; stories.len()];
    for (index, story) in stories.iter().enumerate() {
        let Some(url) = story.get("image").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        let Ok(response) = http.get(url).send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(bytes) = response.bytes().await else {
            continue;
        };
        if !looks_like_image(&bytes) {
            continue;
        }
        let path = work_dir.join(format!("story-image-{index}.img"));
        if std::fs::write(&path, &bytes).is_ok() {
            story_images[index] = Some(path);
        }
    }

    let show_name = api::resolve_show_name(&settings.header);
    let intro_line = api::anchor_line(
        &settings.anchor_intro,
        api::default_intro(&show_name),
        hour,
        minute,
        settings.use_24_hour,
    );
    let outro_line = api::anchor_line(
        &settings.anchor_outro,
        api::default_outro(&show_name),
        hour,
        minute,
        settings.use_24_hour,
    );

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

    // The anchor's copy is read aloud with the TTS provider from the AI page.
    // Without one — or when it fails — Google TTS reads it, so the show still
    // has narration.
    let mut speech: Vec<Option<(PathBuf, f64)>> = vec![None; texts.len()];
    if settings.tts_enabled {
        for (index, text) in texts.iter().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let path = work_dir.join(format!("speech-{index}.mp3"));
            let spoken = match ai_provider.as_ref() {
                Some(provider) => match crate::tts::synthesize_ai(
                    http,
                    text,
                    &provider.base_url,
                    &provider.api_key,
                    &provider.tts_model,
                    &provider.voice,
                    &path,
                )
                .await
                {
                    Ok(count) if count > 0 => Ok(count),
                    _ => crate::tts::synthesize(http, text, &settings.tts_voice, &path).await,
                },
                None => crate::tts::synthesize(http, text, &settings.tts_voice, &path).await,
            };
            if let Ok(count) = spoken {
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

    // After the anchor's close the desktop fades to black, the FlowWire logo
    // fades up over black while the outro music plays, and as the music ends
    // the logo fades back out to black — the video ends as that fade completes.
    let outro_len = probe_seconds(&outro_mp3)
        .filter(|len| *len > 0.5)
        .unwrap_or(OUTRO_SECONDS);
    let total = show_total + outro_len;
    let fade_start = (total - OUTRO_FADE_SECONDS).max(show_total);
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
    // The desktop (map, anchor, bars, captions, mute) is only on screen between
    // the intro and the close: before the lead, and again after the close while
    // the outro music plays, just the FlowWire logo shows over black. The logo
    // eases up from black as the intro music starts, and the last
    // OUTRO_FADE_SECONDS of the video fade the whole picture to black.
    let mut filter_parts = vec![format!(
        "[0:v]trim=duration={total},setpts=PTS-STARTPTS[src];\
         [src]split[base][bg];\
         {map}\
         [2:v]scale=650:650[p];\
         [m2][p]overlay=650:221[l1];\
         [3:v]scale=1920:1080[hb];\
         [l1][hb]overlay=0:0[l2];\
         [4:v]scale=1920:1080[sb];\
         [l2][sb]overlay=0:0[l3];\
         [5:v]scale=1920:1080[mm];\
         [l3][mm]overlay=0:0[desk]",
        total = total,
        map = map_layer(),
    )];
    // The picture inputs sit after the nine fixed inputs (background, map,
    // anchor, headline bar, story bar, mute overlay, logo, intro music, outro
    // music) and one input per spoken segment.
    let mut input_index = 9 + speech.iter().filter(|segment| segment.is_some()).count();
    let mut cur = "desk".to_string();
    let mut picture_slot = 0usize;
    for (index, picture) in story_images.iter().enumerate() {
        if picture.is_none() {
            continue;
        }
        let start = starts.get(index + 1).copied().unwrap_or(lead);
        let end = beats
            .get(index + 1)
            .map(|beat| beat.end)
            .unwrap_or(start + SECONDS_PER_STORY)
            .max(start + 0.5);
        filter_parts.push(story_image_layer(input_index, picture_slot, &cur, start, end));
        cur = format!("picture{picture_slot}");
        input_index += 1;
        picture_slot += 1;
    }
    filter_parts.push(format!(
        "{};{audio}",
        logo_and_outro_filter(
            &cur,
            &escape_filter_path(&ass_path),
            OUTRO_FADE_SECONDS,
            lead,
            show_total,
            fade_start,
        ),
        audio = audio,
    ));
    let filter = filter_parts.join(";");

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
    // add story images as inputs (looped)
    for img in story_images.iter().flatten() {
        args.push("-loop".into());
        args.push("1".into());
        args.push("-framerate".into());
        args.push(FPS.to_string());
        args.push("-t".into());
        args.push(format!("{total}"));
        args.push("-i".into());
        args.push(img.to_string_lossy().to_string());
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

/// The logo eases up from black at the very start of the video, over the opening
/// of the intro music. The outro brings it back with a hard cut instead.
fn logo_fade_in() -> String {
    format!("fade=t=in:st=0:d={INTRO_FADE_SECONDS}")
}

/// The opening and closing chains of the render: the FlowWire logo eases up over
/// black as the intro music starts, holds for the music, then fades back out as
/// the music ends; the desktop (the anchor on the map) then fades up from black
/// and runs through the show; after the anchor's close the desktop fades to
/// black, the FlowWire logo fades up over black for the outro music, and the
/// logo fades back out to black as the music ends. `cur` is the label of the last
/// desktop layer, `ass` the caption file path, `fade` how long the final fade to
/// black takes. The logo stream is split before being handed to the intro and
/// outro overlays: ffmpeg starves a second overlay that reads the same label
/// directly, so without the split the outro logo is never drawn.
fn logo_and_outro_filter(
    cur: &str,
    ass: &str,
    fade: f64,
    lead: f64,
    show_total: f64,
    fade_start: f64,
) -> String {
    // The intro logo fades back out across the last stretch of the intro music,
    // so the picture is black when the anchor comes up.
    let intro_out_dur = INTRO_FADE_SECONDS.min(lead);
    let intro_out_start = (lead - intro_out_dur).max(0.0);
    // The anchor's screen fades to black just before the close, so the outro
    // logo eases up over black instead of a hard cut. The logo's own fade-up is
    // applied to its fork (not the composite, which would black out the show).
    let desk_out_dur = OUTRO_DESKTOP_FADE_SECONDS.min(show_total - lead);
    let desk_out_start = show_total - desk_out_dur;
    let logo_in = OUTRO_LOGO_IN_SECONDS;
    format!(
        "[6:v]scale=560:-1,{intro_fade},split[lg1][lg2];\
         [lg1]fade=t=out:st={intro_out_start}:d={intro_out_dur}[lg_in];\
         [lg2]fade=t=in:st={show_total}:d={logo_in}[lg_out];\
         [base][lg_in]overlay=(W-w)/2:(H-h)/2:enable='lte(t,{lead})'[v1];\
         [{cur}]fade=t=in:st={lead}:d={desk_in}[desk_in];\
         [v1][desk_in]overlay=0:0:enable='between(t,{lead},{show_total})'[v2];\
         [v2]ass='{ass}'[v3];\
         [v3]fade=t=out:st={desk_out_start}:d={desk_out_dur}[v4];\
         [v4][lg_out]overlay=(W-w)/2:(H-h)/2:enable='gte(t,{show_total})',\
         fade=t=out:st={fade_start}:d={fade}[final]",
        intro_fade = logo_fade_in(),
        cur = cur,
        ass = ass,
        fade = fade,
        lead = lead,
        show_total = show_total,
        fade_start = fade_start,
        intro_out_start = intro_out_start,
        intro_out_dur = intro_out_dur,
        desk_out_start = desk_out_start,
        desk_out_dur = desk_out_dur,
        logo_in = logo_in,
        desk_in = DESKTOP_FADE_IN_SECONDS,
    )
}

/// The scrolling-map layer: crop the soft (transparent) right edge off, scale
/// every copy to fill the band between the headline bar and the story bar, split
/// the stream in two, then lay the copies end to end and slide them left. The
/// scroll position wraps at exactly one tile width, so as the first copy leaves
/// the frame the second is already in place and a fresh copy takes over on the
/// right — the map never runs out.
fn map_layer() -> String {
    format!(
        "[1:v]crop={opaque}:{height},scale={tile}:{band},split[ma][mb];\
         [bg][ma]overlay=x='-mod(t*{speed},{tile})':y={top}[m1];\
         [m1][mb]overlay=x='-mod(t*{speed},{tile})+{tile}':y={top}[m2];",
        opaque = MAP_OPAQUE_W,
        height = MAP_SRC_H,
        band = MAP_BAND_H,
        tile = MAP_TILE_W,
        top = MAP_TOP,
        speed = MAP_SCROLL_SPEED,
    )
}

/// Whether the first bytes of a download look like an image ffmpeg can decode,
/// so an HTML error page or a stray video is never fed to the renderer.
fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF]) // jpeg
        || bytes.starts_with(&[0x89, b'P', b'N', b'G']) // png
        || bytes.starts_with(b"GIF8") // gif
        || bytes.starts_with(b"BM") // bmp
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")) // webp
        || bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*") // tiff
}

/// One story's picture, drawn into the anchor's window for that story's window.
/// The picture is scaled to fit `IMAGE_BOX_*` with its aspect ratio intact and
/// centred there, then faded in as the story opens and out as it closes.
fn story_image_layer(input: usize, slot: usize, base: &str, start: f64, end: f64) -> String {
    let fade = IMAGE_FADE_SECONDS;
    let in_start = start.max(0.0);
    let out_start = (end - fade).max(in_start);
    format!(
        "[{input}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,setsar=1,\
         format=rgba,fade=t=in:st={in_start}:d={fade}:alpha=1,\
         fade=t=out:st={out_start}:d={fade}:alpha=1[pic{slot}];\
         [{base}][pic{slot}]overlay=x={x}+({w}-w)/2:y={y}+({h}-h)/2:\
         enable='between(t,{in_start},{end})'[picture{slot}]",
        input = input,
        slot = slot,
        base = base,
        w = IMAGE_BOX_W,
        h = IMAGE_BOX_H,
        x = IMAGE_BOX_X,
        y = IMAGE_BOX_Y,
        in_start = in_start,
        out_start = out_start,
        fade = fade,
        end = end,
    )
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
            if headline_scrolls(&title) {
                // Only genuinely long headlines crawl: they cannot fit on one
                // static line at the floor size, so they slide right-to-left at
                // that size, from fully off the right to fully off the left,
                // finishing as the story ends.
                let width = headline_width(&title, HEADLINE_MIN_PX);
                let x_start = WIDTH as f64 + width / 2.0 + 60.0;
                let x_end = -(width / 2.0) - 60.0;
                let duration_ms = ((beat.end - beat.start).max(1.0) * 1000.0) as i64;
                let scroll_ms = ((title.chars().count() as f64 * 1000.0 / 30.0).max(8000.0) as i64)
                    .min(duration_ms)
                    .max(1);
                let move_tag = format!(
                    "{{\\an5\\move({x_start:.0},{HEADLINE_CENTER_Y},{x_end:.0},{HEADLINE_CENTER_Y},0,{scroll_ms})\\fs{HEADLINE_MIN_PX:.0}}}"
                );
                events.push_str(&format!(
                    "Dialogue: 0,{0},{1},Headline,,0,0,0,,{2}{3}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    move_tag,
                    title
                ));
            } else {
                // A single static line, sized to span the bar's full width and
                // centred in its band — no crawl unless it overflows.
                let font = headline_font_size(&title);
                events.push_str(&format!(
                    "Dialogue: 0,{},{},Headline,,0,0,0,,{{\\an5\\pos(960,{})\\fs{:.0}}}{}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    HEADLINE_CENTER_Y,
                    font,
                    title
                ));
            }
        }
        let body = clean_text(&beat.body);
        if !body.is_empty() && !body.eq_ignore_ascii_case(beat.title.trim()) {
            let (line1, line2) = split_two_lines(&body);
            // The two story areas span nearly the whole frame. Size each line
            // so it fills that width instead of sitting in the centre, and use
            // one size for both so the two lines match.
            let font = story_font_size(line1.chars().count().max(line2.chars().count()));
            let size = format!("{{\\fs{font:.0}}}");
            events.push_str(&format!(
                "Dialogue: 1,{},{},Story1,,0,0,0,,{}{}\n",
                ass_time(beat.start),
                ass_time(beat.end),
                size,
                escape_ass(&line1)
            ));
            if !line2.is_empty() {
                events.push_str(&format!(
                    "Dialogue: 2,{},{},Story2,,0,0,0,,{}{}\n",
                    ass_time(beat.start),
                    ass_time(beat.end),
                    size,
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
    // Headline: a single line, sized per event to fill the bar and centred in
    // its band (the \fs and \an5\pos anchors are set on each event). Story1/
    // Story2: bottom-centre, one line in each marked story area — the first in
    // the green box, the second in the red box at the very bottom.
    ass.push_str("Style: Headline, Liberation Sans, 44, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 1, 0, 0, 0, 100, 100, 0, 0, 1, 3, 0, 5, 40, 40, 0, 1\n");
    ass.push_str(&format!("Style: Story1, Liberation Sans, 32, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 0, 0, 0, 0, 100, 100, 0, 0, 1, 2, 0, 2, 100, 100, {STORY_LINE1_MARGIN_V}, 1\n"));
    ass.push_str(&format!("Style: Story2, Liberation Sans, 32, &H00FFFFFF, &H000000FF, &H00000000, &H80000000, 0, 0, 0, 0, 100, 100, 0, 0, 1, 2, 0, 2, 100, 100, {STORY_LINE2_MARGIN_V}, 1\n\n"));
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


/// Split the story copy into two balanced lines that fill the story bar,
/// breaking at a word boundary near the middle of the copy. The old version
/// capped each line at 48 characters, which left the text stranded in the
/// centre of the wide bar; now the whole summary is used and `build_ass` sizes
/// the lines to span the bar. Only genuinely long copy is trimmed.
fn split_two_lines(text: &str) -> (String, String) {
    const MAX_PER_LINE: usize = 110;
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return (String::new(), String::new());
    }
    let widths: Vec<usize> = words.iter().map(|word| word.chars().count()).collect();
    let total: usize = widths.iter().sum::<usize>() + words.len().saturating_sub(1);
    if total <= MAX_PER_LINE {
        return (words.join(" "), String::new());
    }
    // Cut near the halfway point so the two lines have a similar width.
    let half = total / 2;
    let mut cut = 0;
    let mut used = 0;
    for (index, width) in widths.iter().enumerate() {
        let next = used + if index == 0 { 0 } else { 1 } + width;
        if next > half {
            break;
        }
        used = next;
        cut = index + 1;
    }
    let cut = cut.clamp(1, words.len() - 1);
    let mut line1 = words[..cut].join(" ");
    let mut line2 = words[cut..].join(" ");
    if line1.chars().count() > MAX_PER_LINE {
        line1 = trim_at_word(&line1, MAX_PER_LINE);
    }
    if line2.chars().count() > MAX_PER_LINE {
        line2 = format!("{} …", trim_at_word(&line2, MAX_PER_LINE - 2));
    }
    (line1, line2)
}

/// Cut `text` to at most `max` characters, dropping any partial trailing word.
fn trim_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rsplit_once(' ') {
        Some((head, _)) if !head.is_empty() => head.to_string(),
        _ => cut,
    }
}

/// The story area spans almost the whole 1920-wide frame, so size each story
/// line to fill that width rather than sit in the centre. Liberation Sans
/// advances about 0.44 px per point per character, so a line of N characters
/// needs roughly N * 0.44 * size pixels; solve for the size that reaches the
/// target, capped so the text still fits inside the bars.
fn story_font_size(chars: usize) -> f64 {
    const TARGET_PX: f64 = 1700.0;
    const MIN_PX: f64 = 30.0;
    const MAX_PX: f64 = 52.0;
    if chars == 0 {
        return MAX_PX;
    }
    (TARGET_PX / (chars as f64 * 0.44)).clamp(MIN_PX, MAX_PX)
}

/// Advance widths (per 1000 em) for ASCII 32..=126 in Liberation Sans Bold,
/// read from the installed font. Summing these gives a headline's width far
/// more accurately than a flat per-character factor, so a mixed-case headline
/// fills the bar while an all-caps one is sized down instead of clipping.
const HEADLINE_GLYPH_W: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Size a single headline line to fill the width of the headline bar. Like the
/// story lines, a short headline is grown (up to `HEADLINE_MAX_PX`) so it uses
/// the bar rather than sitting in the centre; a long one is shrunk toward the
/// floor. It is the caller's job to crawl a headline that even the floor cannot
/// fit — see `headline_scrolls`.
fn headline_font_size(text: &str) -> f64 {
    let units = headline_units(text);
    if units <= 0.0 {
        return HEADLINE_MAX_PX;
    }
    (HEADLINE_TARGET_PX * 1000.0 / units).clamp(HEADLINE_MIN_PX, HEADLINE_MAX_PX)
}

/// A headline crawls only when it cannot be shown in full on one static line at
/// the smallest readable size — otherwise the whole bar holds it without moving.
fn headline_scrolls(text: &str) -> bool {
    let units = headline_units(text);
    units > 0.0 && HEADLINE_TARGET_PX * 1000.0 / units < HEADLINE_MIN_PX
}

/// How wide `text` is, in pixels, when drawn at `size` points.
fn headline_width(text: &str, size: f64) -> f64 {
    headline_units(text) * size / 1000.0
}

/// The width of a headline in font units, summed per character. Characters
/// outside printable ASCII fall back to a typical bold letter width (or the
/// space width for other whitespace).
fn headline_units(text: &str) -> f64 {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if (32..=126).contains(&code) {
                HEADLINE_GLYPH_W[(code - 32) as usize] as f64
            } else if c.is_whitespace() {
                278.0
            } else {
                611.0
            }
        })
        .sum()
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
    fn a_normal_headline_fills_the_bar_without_scrolling() {
        // A typical headline is a single static line, centred in the bar's band
        // and sized to span its width instead of sitting at a fixed 44.
        let title = "City council approves budget after a marathon overnight session";
        let beats = vec![Beat {
            start: 0.0,
            end: 8.0,
            title: title.into(),
            body: String::new(),
        }];
        let ass = build_ass(&beats);
        assert!(!ass.contains("\\move"), "a headline that fits does not crawl: {ass}");
        assert!(ass.contains("\\an5\\pos(960,120)"), "it is centred in the bar's band: {ass}");
        let size = headline_font_size(title);
        assert!(ass.contains(&format!("\\fs{size:.0}")), "the fitted size is on the event: {ass}");
        assert!(size > 44.0, "a headline can be bigger than the old fixed 44");
    }

    #[test]
    fn shorter_headlines_are_drawn_bigger_to_fill_the_bar() {
        // Sized against the headline's own glyph widths: more characters means
        // a smaller point size, down to the floor; a short line grows to the
        // cap rather than being stretched.
        assert!(headline_font_size(&"a".repeat(20)) >= headline_font_size(&"a".repeat(60)));
        assert!(headline_font_size(&"a".repeat(60)) > headline_font_size(&"a".repeat(85)));
        for chars in [0, 10, 60, 85, 200] {
            let size = headline_font_size(&"a".repeat(chars));
            assert!(
                (HEADLINE_MIN_PX..=HEADLINE_MAX_PX).contains(&size),
                "font {size} stays in range"
            );
        }
        assert_eq!(headline_font_size(&"a".repeat(10)), HEADLINE_MAX_PX);
    }

    #[test]
    fn only_headlines_that_overflow_the_bar_crawl() {
        // The bar holds roughly sixty of the widest letters ('A') at the floor
        // size, so a headline well past that overflows and has to crawl; a short
        // one stays put on a single static line.
        assert!(!headline_scrolls(&"A".repeat(50)), "a headline that fits stays put");
        assert!(headline_scrolls(&"A".repeat(70)), "one that cannot fit at the floor crawls");
        assert!(!headline_scrolls(""));

        let long = "A".repeat(140);
        let beats = vec![Beat { start: 0.0, end: 12.0, title: long.clone(), body: String::new() }];
        let ass = build_ass(&beats);
        assert!(ass.contains("\\move("), "an over-long headline crawls: {ass}");
        assert!(
            ass.contains(&format!("\\fs{:.0}", HEADLINE_MIN_PX)),
            "it crawls at the floor size: {ass}"
        );
        assert!(headline_width(&long, HEADLINE_MIN_PX) > HEADLINE_TARGET_PX);
    }

    #[test]
    fn headline_sizing_targets_the_bar_width() {
        // The fitted size really does reach the bar for a mid-length headline.
        let text = "City council approves budget after a marathon overnight session";
        let size = headline_font_size(text);
        let width = headline_width(text, size);
        assert!((HEADLINE_TARGET_PX - 1.0..=HEADLINE_TARGET_PX + 1.0).contains(&width));
        // And the vertical centre keeps it inside the bar's band (43–197).
        assert!(HEADLINE_CENTER_Y > 43 && HEADLINE_CENTER_Y < 197);
    }

    #[test]
    fn shorter_copy_gets_a_shorter_window() {
        let short = story_seconds("Short.");
        let long = story_seconds("This is a considerably longer summary with many more words in it.");
        assert!(long > short, "longer copy holds the screen longer");
        assert!(short >= 4.0 && long <= 14.0, "bounded so it cannot stall the show");
    }

    #[test]
    fn story_copy_keeps_the_whole_summary_across_two_lines() {
        // A long RSS summary must not be truncated to 48-character halves.
        let text = "An ICE officer struck a man with an SUV in a Houston parking lot Saturday, \
                    leaving the man with a broken leg after he fled from a vehicle agents were trying to stop.";
        let (line1, line2) = split_two_lines(text);
        assert!(!line2.is_empty(), "a long summary fills both story lines");
        let shown = line1.chars().count() + line2.chars().count();
        assert!(shown >= text.split_whitespace().count() * 4, "most of the copy survives");
        assert!(line1.chars().count() <= 110 && line2.chars().count() <= 112, "lines stay in the bar");
        assert!(!line1.contains('…') && !line2.contains('…'), "a normal summary is not trimmed");
    }

    #[test]
    fn short_copy_stays_on_one_line_but_is_sized_to_fill() {
        let (line1, line2) = split_two_lines("Brief summary.");
        assert_eq!(line2, "", "a short summary is a single line");
        assert_eq!(line1, "Brief summary.");
        // A short line is grown to the maximum so it still fills the bar.
        assert!(story_font_size(line1.chars().count()) > story_font_size(100));
    }

    #[test]
    fn longer_lines_are_drawn_smaller_to_fit_the_bar() {
        assert!(story_font_size(40) >= story_font_size(80));
        assert!(story_font_size(80) >= story_font_size(110));
        // The size always fits inside the story bars (never below the floor).
        for chars in [0, 10, 60, 100, 200] {
            let size = story_font_size(chars);
            assert!((30.0..=52.0).contains(&size), "font {size} stays in range");
        }
    }

    #[test]
    fn story_events_carry_their_fitted_font_size() {
        let beats = vec![Beat {
            start: 0.0,
            end: 5.0,
            title: "Headline One".into(),
            body: "An ICE officer struck a man with an SUV in a Houston parking lot Saturday, leaving the man with a broken leg.".into(),
        }];
        let ass = build_ass(&beats);
        assert!(ass.contains("{\\fs"), "the fitted size is applied to the story line");
        assert!(ass.contains("Story1"), "the first line is in the green story bar");
    }

    #[test]
    fn publishing_a_new_video_removes_the_old_one() {
        let dir = std::env::temp_dir().join(format!("news-old-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("news-20260101-060000.mp4");
        let keep = dir.join("news-20260102-060000.mp4");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&keep, b"new").unwrap();
        // A non-bulletin file beside them must survive the sweep.
        let unrelated = dir.join("notes.txt");
        std::fs::write(&unrelated, b"keep me").unwrap();

        let logger = PluginLogger::new("com.channelflow.news");
        remove_old_videos(&dir, &keep, &logger);

        assert!(!old.exists(), "the previous bulletin is deleted");
        assert!(keep.exists(), "the new bulletin is kept");
        assert!(unrelated.exists(), "unrelated files are left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_map_is_split_so_both_copies_are_drawn() {
        // Reusing one label for both map overlays left the second copy undrawn,
        // so the map ran out mid-build and the frame showed black. The stream
        // must be split, and the two copies laid exactly one tile apart.
        let layer = map_layer();
        assert!(
            layer.contains("split[ma][mb]"),
            "the map must be split before tiling: {layer}"
        );
        assert!(layer.contains("[bg][ma]overlay"), "the first copy is drawn");
        assert!(layer.contains("[m1][mb]overlay"), "the second copy is drawn");
        assert!(
            layer.contains(&format!("+{}", MAP_TILE_W)),
            "the second copy starts one full tile to the right: {layer}"
        );
        // The soft transparent edge is cropped so the copies butt together.
        assert!(
            layer.contains(&format!("crop={MAP_OPAQUE_W}:{MAP_SRC_H}")),
            "the transparent edge is cropped: {layer}"
        );
        // Each copy is scaled to the band between the bars, not the whole
        // screen, and dropped in at the band's top edge.
        assert!(
            layer.contains(&format!("scale={MAP_TILE_W}:{MAP_BAND_H}")),
            "the map is scaled to the band, not the whole screen: {layer}"
        );
        assert!(
            layer.contains(&format!("y={MAP_TOP}")),
            "the map sits at the top of the band: {layer}"
        );
        assert_eq!(
            MAP_TILE_W,
            MAP_OPAQUE_W * MAP_BAND_H / MAP_SRC_H,
            "the tile keeps the map's aspect ratio for the band"
        );
    }

    #[test]
    fn the_logo_comes_back_for_the_outro_then_fades_to_black() {
        let chain = logo_and_outro_filter("desk", "news.ass", OUTRO_FADE_SECONDS, 3.5, 120.0, 129.0);
        // The desktop is up only between the end of the intro music and the
        // anchor's close.
        assert!(
            chain.contains("enable='between(t,3.5,120)'"),
            "the desktop runs only during the show: {chain}"
        );
        // The logo fills the gaps on both sides of the show: up before the lead
        // and back on from the close, while the outro music plays.
        assert!(chain.contains("enable='lte(t,3.5)'"), "the logo is up before the show: {chain}");
        assert!(chain.contains("enable='gte(t,120)'"), "the outro brings the logo back: {chain}");
        // The video runs on past the music so the picture can fade to black,
        // and that fade-out dims the logo as the end approaches.
        assert!(OUTRO_FADE_SECONDS > 0.0, "the outro needs room to fade to black");
        assert!(
            chain.contains(&format!("fade=t=out:st=129:d={OUTRO_FADE_SECONDS}")),
            "the outro fades to black at the end: {chain}"
        );
    }

    #[test]
    fn the_intro_logo_fades_back_out_as_the_music_ends() {
        let chain = logo_and_outro_filter("desk", "news.ass", 1.2, 4.0, 120.0, 129.0);
        // The logo eases up from black over the opening of the intro music.
        assert!(
            chain.contains("fade=t=in:st=0:d=1.2"),
            "the intro logo eases in: {chain}"
        );
        // And it fades back out across the last stretch of the music, before
        // the desktop (and the anchor) comes up — no popped logo under the
        // presenter screen.
        assert!(
            chain.contains("fade=t=out:st=2.8:d=1.2"),
            "the intro logo fades out as the music ends: {chain}"
        );
        assert!(
            chain.contains("enable='between(t,4,120)'"),
            "the presenter screen comes up at the music's end: {chain}"
        );
    }

    #[test]
    fn the_anchor_screen_fades_in_after_the_intro_logo() {
        let chain = logo_and_outro_filter("desk", "news.ass", 1.2, 4.0, 120.0, 129.0);
        // The desktop (the anchor on the map) fades up from black once the
        // intro music is done, instead of popping in.
        assert!(
            chain.contains("[desk]fade=t=in:st=4:d=0.8[desk_in]"),
            "the anchor's screen fades in on its own layer: {chain}"
        );
        assert!(
            chain.contains("[v1][desk_in]overlay=0:0:enable='between(t,4,120)'"),
            "the faded desktop is what gets drawn: {chain}"
        );
        // The fade is on the desktop layer, not the composite: a colour fade on
        // the composite would black out the intro logo that is still up before
        // the lead.
        assert!(
            !chain.contains("enable='between(t,4,120)',fade=t=in"),
            "the fade must not sit on the composite: {chain}"
        );
    }

    #[test]
    fn the_logo_eases_up_at_the_start() {
        // The logo fades in from black over the opening of the intro music
        // rather than popping straight on.
        assert_eq!(logo_fade_in(), "fade=t=in:st=0:d=1.2");
        assert!(
            INTRO_FADE_SECONDS > 0.0,
            "the intro logo should ease in, not pop in"
        );
    }

    #[test]
    fn the_outro_logo_uses_its_own_fork_of_the_logo_stream() {
        let chain = logo_and_outro_filter("desk", "news.ass", 1.2, 3.5, 120.0, 123.5);
        // Two overlays must not read the same label: ffmpeg starves the second
        // one, so without the split the outro logo is never drawn (the outro
        // would stay black while the outro music plays).
        assert!(
            chain.contains(",split[lg1][lg2]"),
            "the logo is split before its two uses: {chain}"
        );
        assert!(
            chain.contains("[base][lg_in]overlay"),
            "the intro overlay reads the first fork: {chain}"
        );
        assert!(
            chain.contains("[v4][lg_out]overlay"),
            "the outro overlay reads the second fork: {chain}"
        );
        // After the anchor's close, the desktop (the presenter on the map)
        // fades to black before the logo comes up — no hard cut.
        assert!(
            chain.contains("fade=t=out:st=119:d=1"),
            "the anchor's screen fades to black first: {chain}"
        );
        // The outro overlay brings the logo back for the music — eased up over
        // black, not popped — and fades it out into black as the music ends.
        assert!(
            chain.contains("enable='gte(t,120)'"),
            "the logo returns for the outro music: {chain}"
        );
        assert!(
            chain.contains("[lg2]fade=t=in:st=120:d=0.8[lg_out]"),
            "the outro logo fades up over black on its own fork: {chain}"
        );
        assert!(
            chain.contains("fade=t=out:st=123.5:d=1.2"),
            "the outro fades the logo out as the music ends: {chain}"
        );
    }

    #[test]
    fn a_story_picture_fits_the_anchor_window_without_stretching() {
        let layer = story_image_layer(9, 0, "desk", 10.0, 16.0);
        // Scaled to fit the window with the aspect ratio kept.
        assert!(
            layer.contains(&format!(
                "scale={IMAGE_BOX_W}:{IMAGE_BOX_H}:force_original_aspect_ratio=decrease"
            )),
            "the picture fits the window without stretching: {layer}"
        );
        // Centred inside the window.
        assert!(
            layer.contains(&format!("x={IMAGE_BOX_X}+({IMAGE_BOX_W}-w)/2")),
            "the picture is centred horizontally: {layer}"
        );
        assert!(
            layer.contains(&format!("y={IMAGE_BOX_Y}+({IMAGE_BOX_H}-h)/2")),
            "the picture is centred vertically: {layer}"
        );
        // Drawn over the desktop (map, mute overlay, anchor) and only while the
        // story is on air.
        assert!(layer.starts_with("[9:v]"), "the picture input is used: {layer}");
        assert!(layer.contains("[desk][pic0]"), "it is drawn over the desktop: {layer}");
        assert!(
            layer.contains("enable='between(t,10,16)'"),
            "it shows only during the story: {layer}"
        );
        // The window is inside the map band, so the bars stay uncovered.
        assert_eq!(IMAGE_BOX_Y, MAP_TOP);
        assert_eq!(IMAGE_BOX_Y + IMAGE_BOX_H, MAP_TOP + MAP_BAND_H);
    }

    #[test]
    fn a_story_picture_fades_in_and_out() {
        let layer = story_image_layer(9, 0, "desk", 10.0, 16.0);
        assert!(
            layer.contains("format=rgba"),
            "an alpha fade needs an alpha channel: {layer}"
        );
        assert!(
            layer.contains("fade=t=in:st=10:d=0.4:alpha=1"),
            "the picture fades up as the story opens: {layer}"
        );
        assert!(
            layer.contains("fade=t=out:st=15.6:d=0.4:alpha=1"),
            "the picture fades back out as the story closes: {layer}"
        );
    }

    #[test]
    fn only_real_images_are_handed_to_the_renderer() {
        assert!(looks_like_image(b"\xff\xd8\xff\xe0 and a jpeg body"));
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\n and a png body"));
        assert!(looks_like_image(b"GIF89a and a gif body"));
        assert!(looks_like_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "));
        assert!(!looks_like_image(b"<html><body>not an image</body></html>"));
        assert!(!looks_like_image(b""));
    }

    #[test]
    fn the_story_lines_sit_down_in_their_boxes() {
        // The copy sits a little lower in the marked story areas than it used
        // to: line 1 was MarginV 144, line 2 was 35.
        assert_eq!(STORY_LINE1_MARGIN_V, 126);
        assert_eq!(STORY_LINE2_MARGIN_V, 17);
        // The two lines keep their spacing and the second stays on screen.
        assert!(STORY_LINE1_MARGIN_V > STORY_LINE2_MARGIN_V);
        assert!(STORY_LINE2_MARGIN_V > 0);
    }
}
