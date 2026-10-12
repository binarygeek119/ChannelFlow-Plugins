//! The build loop for one `(channel, date)`: compile the layers, ask the model,
//! parse its answer, validate it, repair up to `max_retries`, then fall back to
//! a deterministic day so a channel is never empty.

use std::sync::Arc;

use channelflow_plugin_api::core::{CatalogFilter, CatalogRow, ChatMessage, ChatRequest, CoreData};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::layers::{self, ItemsLayer, ModelOutput, PoolItem, ResolvedLayers, Rule};
use crate::settings::SchedulerSettings;

/// One item placed in a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiredItem {
    pub match_key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// One block of the materialised day. `start_ms`/`finish_ms` are milliseconds
/// from local midnight of the date; `filled_ms` is the runtime of its items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiredBlock {
    pub slot_index: i32,
    pub span_slots: i32,
    pub start_ms: i64,
    pub finish_ms: i64,
    #[serde(default)]
    pub filled_ms: i64,
    pub items: Vec<AiredItem>,
}

/// A whole generated day, stored with the hashes that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedDay {
    pub channel_id: String,
    pub date: String,
    pub base_hash: String,
    pub input_hash: String,
    pub blocks: Vec<AiredBlock>,
    /// True when the model could not produce a valid day and the deterministic
    /// fallback filled it instead.
    #[serde(default)]
    pub degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub validation: Vec<String>,
    /// The exact prompt sent (kept so the UI can show what the model saw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// What one build produced.
#[derive(Debug, Clone, Serialize)]
pub struct BuildOutcome {
    pub day: GeneratedDay,
    /// True when the stored day already matched the input hash, so no call was
    /// made and nothing changed.
    pub skipped: bool,
    /// How many chat turns were spent (0 when skipped or on a fallback).
    pub attempts: u32,
}

/// Map the items layer onto the core's catalog filter. List ids are not resolved
/// yet (a later phase reads them from the Lists plugin).
pub fn catalog_filter(items: &ItemsLayer) -> CatalogFilter {
    let mut match_keys = items.match_keys.clone();
    for pin in &items.pins {
        if !pin.exclude {
            match_keys.push(pin.match_key.clone());
        }
    }
    CatalogFilter {
        kinds: items.filter.kinds.clone(),
        genres: items.filter.genres.clone(),
        tags: items.filter.tags.clone(),
        year_min: items.filter.year_min,
        year_max: items.filter.year_max,
        rating_max: items.filter.rating_max.clone(),
        match_keys,
        limit: None,
    }
}

/// Take up to `cap` rows, round-robin across kinds when the filter names more
/// than one, so a "TV and movies" pool is not all one kind.
fn interleave(rows: Vec<CatalogRow>, kinds: &[String], cap: usize) -> Vec<PoolItem> {
    let mut buckets: Vec<Vec<CatalogRow>> = Vec::new();
    if kinds.len() > 1 {
        for kind in kinds {
            buckets.push(
                rows.iter()
                    .filter(|row| &row.kind == kind)
                    .cloned()
                    .collect(),
            );
        }
    } else {
        buckets.push(rows);
    }
    let mut out = Vec::new();
    let mut index = 0;
    while out.len() < cap {
        let mut progressed = false;
        for bucket in &buckets {
            if let Some(row) = bucket.get(index) {
                progressed = true;
                out.push(PoolItem {
                    match_key: row.match_key.clone(),
                    kind: row.kind.clone(),
                    title: row.title.clone(),
                    runtime_ms: row.runtime_ms,
                });
                if out.len() >= cap {
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
        index += 1;
    }
    out
}

/// The MaxPerDay rules, as `(kind, n)` pairs the validator checks.
fn max_per_day(rules: &[Rule]) -> Vec<(String, u32)> {    rules
        .iter()
        .filter_map(|rule| match rule {
            Rule::MaxPerDay { kind, n } => Some((kind.clone(), *n)),
            _ => None,
        })
        .collect()
}

/// A hash of everything that would change the answer: the base contract, the
/// full prompt, and the candidate pool (ids and runtimes). Rebuilding a day with
/// the same hash skips the model entirely.
fn input_hash(compiled: &layers::Compiled) -> String {
    let mut hasher = Sha256::new();
    hasher.update(compiled.base_hash.as_bytes());
    hasher.update(compiled.prompt.as_bytes());
    for item in &compiled.pool {
        hasher.update(item.match_key.as_bytes());
        hasher.update(item.runtime_ms.unwrap_or(0).to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// The local minute-of-day the broadcast day starts at (`HH:MM`, default 0).
fn day_start_minutes(hours: &layers::HoursLayer) -> i64 {
    let mut parts = hours.day_start.trim().split(':');
    let hour = parts.next().and_then(|h| h.parse::<i64>().ok()).unwrap_or(0);
    let minute = parts.next().and_then(|m| m.parse::<i64>().ok()).unwrap_or(0);
    hour.clamp(0, 23) * 60 + minute.clamp(0, 59)
}

/// The first complete JSON object in a reply (models often wrap it in prose or
/// a code fence). String-aware brace matching, so braces inside strings do not
/// end it early.
pub fn first_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        let ch = *byte;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == b'\\' {
                escaped = true;
            } else if ch == b'"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + 1;
                    return serde_json::from_str(&text[start..end]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

fn materialise(
    channel_id: &str,
    date: &str,
    compiled: &layers::Compiled,
    output: &ModelOutput,
    degraded: bool,
    provider: Option<String>,
    validation: Vec<String>,
    hash: String,
    now: String,
) -> GeneratedDay {
    let day_start = day_start_minutes(&compiled.layers.hours);
    let blocks = output
        .blocks
        .iter()
        .map(|block| {
            let start_ms = (day_start + block.slot_index as i64 * 30) * 60_000;
            let finish_ms = start_ms + block.span_slots.max(1) as i64 * 30 * 60_000;
            let items: Vec<AiredItem> = block
                .items
                .iter()
                .map(|item| {
                    let found = compiled.pool.iter().find(|pool| pool.match_key == item.id);
                    AiredItem {
                        match_key: item.id.clone(),
                        title: found.map(|pool| pool.title.clone()).unwrap_or_default(),
                        runtime_ms: found.and_then(|pool| pool.runtime_ms),
                        why: item.why.clone(),
                    }
                })
                .collect();
            let filled_ms = items.iter().filter_map(|item| item.runtime_ms).sum();
            AiredBlock {
                slot_index: block.slot_index,
                span_slots: block.span_slots.max(1),
                start_ms,
                finish_ms,
                filled_ms,
                items,
            }
        })
        .collect();
    GeneratedDay {
        channel_id: channel_id.to_string(),
        date: date.to_string(),
        base_hash: compiled.base_hash.clone(),
        input_hash: hash,
        blocks,
        degraded,
        provider,
        created_at: now,
        validation,
        prompt: Some(compiled.prompt.clone()),
    }
}

/// A deterministic day that fills the whole broadcast day from the pool, so a
/// channel is never empty when the model fails. Twelve two-hour blocks, one
/// item each, chosen round-robin.
fn fallback_day(
    channel_id: &str,
    date: &str,
    compiled: &layers::Compiled,
    validation: Vec<String>,
    provider: Option<String>,
    hash: String,
    now: String,
) -> GeneratedDay {
    let day_start = day_start_minutes(&compiled.layers.hours);
    let blocks = if compiled.pool.is_empty() {
        vec![AiredBlock {
            slot_index: 0,
            span_slots: 48,
            start_ms: day_start * 60_000,
            finish_ms: (day_start + 48 * 30) * 60_000,
            filled_ms: 0,
            items: Vec::new(),
        }]
    } else {
        (0..12)
            .map(|index| {
                let item = &compiled.pool[index % compiled.pool.len()];
                let slot = index as i64 * 4;
                let start_ms = (day_start + slot * 30) * 60_000;
                AiredBlock {
                    slot_index: slot as i32,
                    span_slots: 4,
                    start_ms,
                    finish_ms: start_ms + 4 * 30 * 60_000,
                    filled_ms: item.runtime_ms.unwrap_or(0),
                    items: vec![AiredItem {
                        match_key: item.match_key.clone(),
                        title: item.title.clone(),
                        runtime_ms: item.runtime_ms,
                        why: "deterministic fallback".to_string(),
                    }],
                }
            })
            .collect()
    };
    GeneratedDay {
        channel_id: channel_id.to_string(),
        date: date.to_string(),
        base_hash: compiled.base_hash.clone(),
        input_hash: hash,
        blocks,
        degraded: true,
        provider,
        created_at: now,
        validation,
        prompt: Some(compiled.prompt.clone()),
    }
}

/// Build one day. `previous` is the stored day, if any: when its input hash
/// matches, the call is skipped and the stored day returned unchanged.
pub async fn build_day(
    core: &Arc<dyn CoreData>,
    channel_id: &str,
    date: &str,
    layers: ResolvedLayers,
    settings: &SchedulerSettings,
    previous: Option<&GeneratedDay>,
) -> Result<BuildOutcome, String> {
    let rows = core
        .catalog(catalog_filter(&layers.items))
        .await
        .map_err(|error| error.to_string())?;
    let pool = interleave(rows, &layers.items.filter.kinds, settings.pool_cap as usize);
    let compiled = layers::compile(layers, pool);
    let hash = input_hash(&compiled);
    let now = chrono::Local::now().to_rfc3339();

    if let Some(previous) = previous {
        if previous.input_hash == hash {
            return Ok(BuildOutcome {
                day: previous.clone(),
                skipped: true,
                attempts: 0,
            });
        }
    }

    let limits = max_per_day(&compiled.layers.rules);
    let mut messages = vec![ChatMessage {
        role: "user".to_string(),
        content: compiled.prompt.clone(),
    }];
    let mut validation: Vec<String> = Vec::new();
    let mut provider: Option<String> = None;
    let mut attempts = 0u32;

    for attempt in 0..=settings.max_retries {
        attempts = attempt + 1;
        let response = match core
            .chat(ChatRequest {
                messages: messages.clone(),
                temperature: Some(0.2),
                json: true,
                model: None,
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                validation = vec![error.to_string()];
                break;
            }
        };
        provider = Some(response.provider_id.clone());
        match first_json_object(&response.content) {
            Some(value) => match serde_json::from_value::<ModelOutput>(value) {
                Ok(output) => {
                    let errors = layers::validate(&output, &compiled.pool, &limits);
                    if errors.is_empty() {
                        let day = materialise(
                            channel_id, date, &compiled, &output, false, provider.clone(),
                            validation.clone(), hash, now,
                        );
                        return Ok(BuildOutcome { day, skipped: false, attempts });
                    }
                    tracing::warn!(
                        reply = %response.content.chars().take(600).collect::<String>(),
                        errors = %errors.join("; "),
                        "scheduler: the model's day failed validation"
                    );
                    validation = errors.clone();
                    messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: response.content.clone(),
                    });
                    messages.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!(
                            "Your answer had problems: {}. Reply with a corrected JSON object only.",
                            errors.join("; ")
                        ),
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        reply = %response.content.chars().take(600).collect::<String>(),
                        error = %error,
                        "scheduler: the reply did not match the model shape"
                    );
                    validation = vec![error.to_string()];
                    messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: response.content.clone(),
                    });
                    messages.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!(
                            "That did not match the required shape ({error}). Reply with the JSON object only."
                        ),
                    });
                }
            },
            None => {
                tracing::warn!(
                    reply = %response.content.chars().take(600).collect::<String>(),
                    "scheduler: the reply held no JSON object"
                );
                validation = vec!["the reply held no JSON object".to_string()];
                messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: response.content.clone(),
                });
                messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: "Reply with the JSON object only, and nothing else.".to_string(),
                });
            }
        }
    }

    let day = fallback_day(channel_id, date, &compiled, validation, provider, hash, now);
    Ok(BuildOutcome { day, skipped: false, attempts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelflow_plugin_api::core::{CatalogRow, CoreChannel, CoreDataError};
    use serde_json::json;
    use std::sync::Mutex;

    struct FakeCore {
        pool: Vec<CatalogRow>,
        replies: Mutex<Vec<String>>,
        calls: Mutex<u32>,
    }

    impl FakeCore {
        fn new(replies: Vec<&str>) -> Self {
            Self {
                pool: vec![
                    CatalogRow {
                        match_key: "a".into(), kind: "movie".into(), title: "A".into(),
                        year: Some(2000), runtime_ms: Some(5_400_000), genres: vec![],
                        official_rating: None, tags: vec![], series_key: None, season: None, episode: None,
                    },
                    CatalogRow {
                        match_key: "b".into(), kind: "movie".into(), title: "B".into(),
                        year: Some(2001), runtime_ms: Some(6_000_000), genres: vec![],
                        official_rating: None, tags: vec![], series_key: None, season: None, episode: None,
                    },
                ],
                replies: Mutex::new(replies.into_iter().map(str::to_string).collect()),
                calls: Mutex::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl CoreData for FakeCore {
        async fn channels(&self) -> Result<Vec<CoreChannel>, CoreDataError> {
            Ok(Vec::new())
        }
        async fn installed_plugins(&self) -> Result<Vec<String>, CoreDataError> {
            Ok(Vec::new())
        }
        async fn catalog(&self, _filter: CatalogFilter) -> Result<Vec<CatalogRow>, CoreDataError> {
            Ok(self.pool.clone())
        }
        async fn chat(
            &self,
            _request: ChatRequest,
        ) -> Result<channelflow_plugin_api::core::ChatResponse, CoreDataError> {
            *self.calls.lock().unwrap() += 1;
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Err(CoreDataError("the fake has no reply left".to_string()));
            }
            Ok(channelflow_plugin_api::core::ChatResponse {
                content: replies.remove(0),
                provider_id: "fake".to_string(),
                attempts: Vec::new(),
            })
        }
    }

    fn full_day(ids: &[&str]) -> String {
        json!({ "blocks": ids
            .iter()
            .enumerate()
            .map(|(index, id)| json!({
                "slot_index": index as i32 * 24,
                "span_slots": 24,
                "items": [ { "id": id, "why": "test" } ],
            }))
            .collect::<Vec<_>>() })
        .to_string()
    }

    fn crow(key: &str, kind: &str) -> CatalogRow {
        CatalogRow {
            match_key: key.into(),
            kind: kind.into(),
            title: key.into(),
            year: None,
            runtime_ms: None,
            genres: vec![],
            official_rating: None,
            tags: vec![],
            series_key: None,
            season: None,
            episode: None,
        }
    }

    #[test]
    fn a_multi_kind_pool_is_interleaved() {
        let rows = vec![crow("m1", "movie"), crow("m2", "movie"), crow("s1", "series")];
        let pool = interleave(rows, &["movie".to_string(), "series".to_string()], 3);
        let kinds: Vec<&str> = pool.iter().map(|item| item.kind.as_str()).collect();
        assert_eq!(kinds, ["movie", "series", "movie"], "the pool mixes kinds");
    }

    fn settings() -> SchedulerSettings {
        SchedulerSettings::default()
    }

    #[tokio::test]
    async fn a_broken_reply_is_repaired_then_accepted() {
        let core: Arc<dyn CoreData> = Arc::new(FakeCore::new(vec![
            // Overlaps and leaves the day short: the validator rejects it.
            r#"{"blocks":[{"slot_index":0,"span_slots":24,"items":[{"id":"a"}]}]}"#,
            &full_day(&["a", "b"]),
        ]));
        let outcome = build_day(&core, "chan", "2026-01-01", ResolvedLayers::default(), &settings(), None)
            .await
            .expect("build");
        assert!(!outcome.day.degraded, "the repaired answer is used");
        assert_eq!(outcome.day.blocks.len(), 2);
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.day.provider.as_deref(), Some("fake"));
        assert!(!outcome.day.input_hash.is_empty());
    }

    #[tokio::test]
    async fn a_dead_provider_falls_back_to_a_full_deterministic_day() {
        let core: Arc<dyn CoreData> = Arc::new(FakeCore::new(vec![]));
        let outcome = build_day(&core, "chan", "2026-01-01", ResolvedLayers::default(), &settings(), None)
            .await
            .expect("build");
        assert!(outcome.day.degraded, "no valid answer means the fallback");
        assert_eq!(outcome.day.blocks.len(), 12);
        assert!(outcome.day.blocks.iter().all(|block| !block.items.is_empty()));
        assert!(outcome.day.validation.iter().any(|line| line.contains("no reply")));
    }

    #[tokio::test]
    async fn an_unchanged_input_skips_the_model() {
        let core = Arc::new(FakeCore::new(vec![&full_day(&["a", "b"])]));
        let dyn_core: Arc<dyn CoreData> = core.clone();
        let first = build_day(&dyn_core, "chan", "2026-01-01", ResolvedLayers::default(), &settings(), None)
            .await
            .expect("first");
        assert!(!first.skipped);
        let calls_after_first = *core.calls.lock().unwrap();
        let second = build_day(
            &dyn_core, "chan", "2026-01-01", ResolvedLayers::default(), &settings(), Some(&first.day),
        )
        .await
        .expect("second");
        assert!(second.skipped, "the same input hash skips the model");
        assert_eq!(*core.calls.lock().unwrap(), calls_after_first, "no extra call");
    }

    #[test]
    fn the_first_json_object_survives_prose_and_strings() {
        let value = first_json_object("Sure! {\"blocks\":[]} hope that helps").expect("object");
        assert_eq!(value["blocks"], json!([]));
        let nested = first_json_object("```json\n{\"a\":\"}\",\"blocks\":[]}\n```").expect("object");
        assert_eq!(nested["blocks"], json!([]));
        assert!(first_json_object("no object here").is_none());
    }
}
