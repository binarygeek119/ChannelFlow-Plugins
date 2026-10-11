//! Google TTS for the newscast, ported from 1.0.0's NewsTtsService.
//!
//! The free `translate.google.com/translate_tts` endpoint reads a short line
//! of text aloud; long scripts are chunked at sentence boundaries (≤180
//! letters) and the clips are stitched with ffmpeg into one mp3. A
//! rate-limited reply stops the run rather than retrying in a loop — the
//! bulletin falls back to a silent show.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Settings' TTS engine value for Google.
pub fn is_engine(engine: &str) -> bool {
    engine.trim().eq_ignore_ascii_case("google")
}

/// The longest line Google's endpoint comfortably reads at once.
pub const MAX_CHUNK: usize = 180;

/// Split a script into ≤`max`-character chunks at sentence breaks.
pub fn chunk(text: &str, max: usize) -> Vec<String> {
    let cleaned = text.replace('\n', " ").trim().to_string();
    if cleaned.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut remaining = cleaned.as_str();
    while !remaining.is_empty() {
        if remaining.len() <= max {
            out.push(remaining.trim().to_string());
            break;
        }
        let window = &remaining[..max];
        let split = window
            .rfind(". ")
            .or_else(|| window.rfind(' '))
            .map(|at| at)
            .filter(|at| *at >= 20)
            .unwrap_or(max);
        out.push(remaining[..split].trim().to_string());
        remaining = remaining[split..].trim_start_matches(['.', ' ']);
    }
    out.retain(|part| !part.is_empty());
    out
}

/// The two-letter language Google's TTS wants from a voice id like `en-US`.
pub fn google_lang(voice: &str) -> String {
    let trimmed = voice.trim();
    if trimmed.is_empty() {
        return "en".to_string();
    }
    let lang = trimmed.split('-').next().unwrap_or(trimmed);
    lang.to_ascii_lowercase()
}

/// Synthesize `text` as an mp3 at `out`. Returns the number of chunks read
/// (0 when nothing was spoken).
pub async fn synthesize(
    http: &reqwest::Client,
    text: &str,
    voice: &str,
    out: &Path,
) -> Result<usize, String> {
    let chunks = chunk(text, MAX_CHUNK);
    if chunks.is_empty() {
        return Err("nothing to read".to_string());
    }
    let work = out.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let lang = google_lang(voice);
    let mut parts = Vec::new();

    for (index, part) in chunks.iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let url = format!(
            "https://translate.google.com/translate_tts?ie=UTF-8&q={}&tl={}&client=tw-ob",
            url_encode(part),
            url_encode(&lang)
        );
        let response = http
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (ChannelFlow news TTS)")
            .header("Referer", "https://translate.google.com/")
            .send()
            .await
            .map_err(|error| format!("could not reach Google TTS: {error}"))?;
        if response.status().as_u16() == 429 {
            return Err("Google TTS rate-limited; the newscast plays without speech.".to_string());
        }
        if !response.status().is_success() {
            return Err(format!("Google TTS answered {}", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "could not read the speech clip".to_string())?;
        if bytes.len() < 64 {
            return Err("Google TTS returned an empty clip.".to_string());
        }
        let path = work.join(format!("part-{index:02}.mp3"));
        std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
        parts.push(path);
    }

    if parts.len() == 1 {
        std::fs::copy(&parts[0], out).map_err(|error| error.to_string())?;
        return Ok(parts.len());
    }

    let list_path = work.join("concat.txt");
    let mut list = String::new();
    for part in &parts {
        let name = part.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        list.push_str(&format!("file '{name}'\n"));
    }
    std::fs::write(&list_path, list).map_err(|error| error.to_string())?;
    let output = Command::new(encoder_path())
        .current_dir(&work)
        .args(["-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", "concat.txt", "-c:a", "libmp3lame", "-b:a", "64k", "-y"])
        .arg(out)
        .output()
        .map_err(|error| format!("could not run ffmpeg: {error}"))?;
    if !output.status.success() {
        return Err("ffmpeg could not stitch the speech clips".to_string());
    }
    if out.exists() && out.metadata().map(|meta| meta.len()).unwrap_or(0) > 0 {
        Ok(parts.len())
    } else {
        Err("no speech clip was written".to_string())
    }
}

/// OpenAI-compatible TTS caps at much longer chunks than Google's endpoint.
pub const AI_MAX_CHUNK: usize = 4000;

/// Synthesize `text` through an OpenAI-compatible `/audio/speech` endpoint
/// (the AI engine), returning the number of chunks read.
pub async fn synthesize_ai(
    http: &reqwest::Client,
    text: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
    voice: &str,
    out: &Path,
) -> Result<usize, String> {
    if base_url.trim().is_empty() {
        return Err("no AI TTS endpoint is set".to_string());
    }
    let chunks = chunk(text, AI_MAX_CHUNK);
    if chunks.is_empty() {
        return Err("nothing to read".to_string());
    }
    let work = out.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let mut parts = Vec::new();
    for (index, part) in chunks.iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let mut request = http.post(format!("{}/audio/speech", base_url.trim_end_matches('/')));
        if !api_key.trim().is_empty() {
            request = request.bearer_auth(api_key.trim());
        }
        let body = serde_json::json!({
            "model": model.trim(),
            "input": part.trim(),
            "voice": voice.trim(),
            "response_format": "mp3",
        });
        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("could not reach the AI TTS endpoint: {error}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(format!("AI TTS answered {status}: {}", truncate(&detail)));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "could not read the speech clip".to_string())?;
        if bytes.len() < 64 {
            return Err("AI TTS returned an empty clip.".to_string());
        }
        let path = work.join(format!("ai-part-{index:02}.mp3"));
        std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
        parts.push(path);
    }

    if parts.len() == 1 {
        std::fs::copy(&parts[0], out).map_err(|error| error.to_string())?;
        return Ok(parts.len());
    }

    let list_path = work.join("concat.txt");
    let mut list = String::new();
    for part in &parts {
        let name = part.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        list.push_str(&format!("file '{name}'\n"));
    }
    std::fs::write(&list_path, list).map_err(|error| error.to_string())?;
    let output = Command::new(encoder_path())
        .current_dir(&work)
        .args(["-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", "concat.txt", "-c:a", "libmp3lame", "-b:a", "64k", "-y"])
        .arg(out)
        .output()
        .map_err(|error| format!("could not run ffmpeg: {error}"))?;
    if !output.status.success() {
        return Err("ffmpeg could not stitch the speech clips".to_string());
    }
    if out.exists() && out.metadata().map(|meta| meta.len()).unwrap_or(0) > 0 {
        Ok(parts.len())
    } else {
        Err("no speech clip was written".to_string())
    }
}

fn truncate(text: &str) -> String {
    let head: String = text.chars().take(140).collect();
    if text.chars().count() > 140 {
        format!("{head}…")
    } else {
        head
    }
}

fn encoder_path() -> PathBuf {
    let bundled = PathBuf::from("/usr/local/bin/ffmpeg");
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from("ffmpeg")
    }
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_at_sentence_breaks_within_the_cap() {
        let text = "a ".repeat(300) + "long script";
        let chunks = chunk(&text, MAX_CHUNK);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= MAX_CHUNK));
    }

    #[test]
    fn voice_ids_map_to_languages() {
        assert_eq!(google_lang("en-US"), "en");
        assert_eq!(google_lang("es"), "es");
        assert_eq!(google_lang(""), "en");
    }
}