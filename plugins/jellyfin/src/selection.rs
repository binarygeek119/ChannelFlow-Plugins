//! File selection: filter the candidate files for an item, score them
//! against playback preferences, and pick a deterministic winner. Never a
//! bare `ORDER BY`.

use serde::{Deserialize, Serialize};

/// How a file should be preferred during playback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackPrefs {
    /// default: highest
    pub resolution: ResolutionPref,
    pub prefer_codec: CodecPref,
    pub prefer_hdr: bool,
    /// connection ids in priority order, best first
    pub connection_priority: Vec<i64>,
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        Self {
            resolution: ResolutionPref::Highest,
            prefer_codec: CodecPref::Any,
            prefer_hdr: true,
            connection_priority: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionPref {
    Highest,
    P1080,
    P720,
    Smallest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecPref {
    H265,
    H264,
    Av1,
    Any,
}

/// A candidate file for one item.
pub struct Candidate {
    pub file_id: i64,
    pub source_id: i64,
    pub connection_id: i64,
    pub height: Option<i32>,
    pub video_codec: Option<String>,
    pub hdr_format: Option<String>,
    pub is_remote: bool,
    pub exists: bool,
    /// Preferred audio languages; unused by the current scoring, kept because
    /// preferences evolve.
    #[allow(dead_code)]
    pub languages: Vec<String>,
}

/// Filter → score → deterministic tie-break.
pub fn select(cands: &[Candidate], prefs: &PlaybackPrefs) -> Option<i64> {
    let mut scored: Vec<(i64, i64, i64)> = cands
        .iter()
        .map(|c| {
            let mut s = 0i64;
            s += res_weight(c.height, prefs.resolution);
            s += codec_weight(c.video_codec.as_deref(), prefs.prefer_codec);
            if prefs.prefer_hdr
                && c.hdr_format.as_deref().is_some_and(|h| h != "SDR")
            {
                s += 15;
            }
            if c.exists {
                s += 1000;
            } else {
                s -= 500;
            }
            if c.is_remote {
                s -= 50;
            }
            if let Some(pos) = prefs
                .connection_priority
                .iter()
                .position(|id| *id == c.connection_id)
            {
                s += (100 - pos as i64).max(0);
            }
            // sort key: the highest score first; ties resolve by source then
            // file id, so the same inputs always pick the same file.
            (-s, c.source_id, c.file_id)
        })
        .collect();
    scored.sort();
    scored.first().map(|(_, _, id)| *id)
}

fn res_weight(h: Option<i32>, pref: ResolutionPref) -> i64 {
    let h = h.unwrap_or(0);
    match pref {
        ResolutionPref::Highest => (h as i64) / 50,
        ResolutionPref::Smallest => 100 - (h as i64) / 50,
        ResolutionPref::P1080 => {
            if h >= 1080 {
                40
            } else {
                h as i64 / 100
            }
        }
        ResolutionPref::P720 => {
            if h >= 720 {
                40
            } else {
                h as i64 / 100
            }
        }
    }
}

fn codec_weight(codec: Option<&str>, pref: CodecPref) -> i64 {
    match (pref, codec) {
        (CodecPref::Any, _) => 0,
        (CodecPref::H265, Some("hevc")) => 30,
        (CodecPref::H264, Some("h264")) => 30,
        (CodecPref::Av1, Some("av1")) => 30,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: i64, h: i32, exists: bool) -> Candidate {
        Candidate {
            file_id: id,
            source_id: id,
            connection_id: 1,
            height: Some(h),
            video_codec: Some("hevc".into()),
            hdr_format: Some("SDR".into()),
            is_remote: false,
            exists,
            languages: vec![],
        }
    }

    #[test]
    fn existence_beats_quality() {
        let prefs = PlaybackPrefs::default();
        let picked = select(&[c(1, 2160, false), c(2, 1080, true)], &prefs);
        assert_eq!(picked, Some(2));
    }

    #[test]
    fn highest_wins_when_both_exist() {
        let prefs = PlaybackPrefs::default();
        assert_eq!(select(&[c(1, 2160, true), c(2, 1080, true)], &prefs), Some(1));
    }

    #[test]
    fn tie_breaks_deterministically() {
        let prefs = PlaybackPrefs::default();
        assert_eq!(select(&[c(3, 1080, true), c(1, 1080, true)], &prefs), Some(1));
    }

    #[test]
    fn hdr_bonus_breaks_a_tie() {
        let mut hdr = c(2, 1080, true);
        hdr.hdr_format = Some("HDR10".into());
        let sdr = c(1, 1080, true);
        assert_eq!(select(&[sdr, hdr], &PlaybackPrefs::default()), Some(2));
    }
}