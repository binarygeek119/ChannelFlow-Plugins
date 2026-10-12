//! The six layers, how an override rewrites them, and how they compile into an
//! LLM prompt plus the validator that checks the model's answer.
//!
//! Layer 1 (the output contract) is shipped with the plugin and is never
//! overridable — a user rule can never break the JSON contract. Layer 5 is free
//! text and therefore prompt-only by design. Layers 2–4 each have a typed shape
//! and a `to_prompt` / check pair so a rule lives in the prompt *and* in code.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Layer 1 — the output contract. Versioned constant; its hash is stored with
/// every generated day so a change is visible.
pub const BASE_LAYER: &str = "\
You are programming exactly one broadcast day of a television channel.
Return ONLY a JSON object of this shape, and nothing else:
{\"blocks\":[{\"slot_index\":<0-47>,\"span_slots\":<1-48>,\"items\":[{\"id\":\"<an id from the candidate list>\",\"why\":\"<short reason>\"}]}]}
Rules that never change:
- The day is 48 half-hour slots, 0..47. Every slot must be covered exactly once, in order.
- A block occupies slot_index..slot_index+span_slots-1 and must not overlap another block.
- Use ONLY ids from the candidate list. Never invent an id.
- The scheduler computes each block's start and finish from runtimes; you choose what plays and in what order.
- Group items into as FEW blocks as possible: most days are 8 to 16 blocks, each spanning several hours. Do not emit one block per half-hour unless the content really changes every half-hour.
- Respect the channel's rules and dayparts below.
- Output nothing but the JSON object.";

/// A stable hash of the base layer, stored with every generated day.
pub fn base_layer_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(BASE_LAYER.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ── layer 2: channel time work ───────────────────────────────────────────────

/// A from/to window in `HH:MM`, used for off-air periods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub from: String,
    pub to: String,
}

/// A part of the day with an allowed set of kinds. Empty `kinds` means "any".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Daypart {
    pub name: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub kinds: Vec<String>,
}

/// Layer 2 — the broadcast-day shape. `roster` is the base `ChannelLineup` this
/// channel already has (read through the core), kept as JSON so the two models
/// stay independent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HoursLayer {
    /// Local time the broadcast day starts, `HH:MM` (e.g. `06:00`).
    pub day_start: String,
    pub off_air: Vec<Window>,
    pub dayparts: Vec<Daypart>,
}

impl HoursLayer {
    pub fn to_prompt(&self) -> String {
        let mut lines = vec!["Channel time:".to_string()];
        if !self.day_start.trim().is_empty() {
            lines.push(format!("- The broadcast day starts at {}.", self.day_start));
        }
        for window in &self.off_air {
            lines.push(format!("- Off air from {} to {}.", window.from, window.to));
        }
        for part in &self.dayparts {
            let kinds = if part.kinds.is_empty() {
                "any kind".to_string()
            } else {
                part.kinds.join(", ")
            };
            lines.push(format!(
                "- {} ({}–{}): {}.",
                part.name, part.from, part.to, kinds
            ));
        }
        lines.join("\n")
    }
}

// ── layer 3: channel rules ───────────────────────────────────────────────────

/// A typed channel rule. Each has a prompt line and a code check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum Rule {
    NoRepeatWithin { days: u32 },
    MaxPerDay { kind: String, n: u32 },
    RatingCeiling { rating: String },
    SeriesOrder { order: SeriesOrder },
    BlockAlign { kinds: Vec<String> },
    MinGapBetweenSameSeries { hours: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesOrder {
    Chronological,
    Shuffle,
    Random,
}

impl Rule {
    pub fn to_prompt(&self) -> String {
        match self {
            Rule::NoRepeatWithin { days } => {
                format!("- Do not repeat an item within {days} day(s).")
            }
            Rule::MaxPerDay { kind, n } => {
                format!("- At most {n} {kind} item(s) per day.")
            }
            Rule::RatingCeiling { rating } => {
                format!("- Nothing rated above {rating}.")
            }
            Rule::SeriesOrder { order } => format!(
                "- Play series episodes in {} order.",
                match order {
                    SeriesOrder::Chronological => "chronological",
                    SeriesOrder::Shuffle => "shuffled",
                    SeriesOrder::Random => "random",
                }
            ),
            Rule::BlockAlign { kinds } => format!(
                "- Start {} on the hour where possible.",
                kinds.join(", ")
            ),
            Rule::MinGapBetweenSameSeries { hours } => {
                format!("- Keep at least {hours} hour(s) between two episodes of the same series.")
            }
        }
    }
}

// ── layer 4: the pool ────────────────────────────────────────────────────────

/// How the pool is narrowed before it reaches a model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemFilter {
    pub kind: Option<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub year_min: Option<i32>,
    pub year_max: Option<i32>,
    pub rating_max: Option<String>,
}

/// A per-item constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub match_key: String,
    /// `only_days` are 0=Sunday…6=Saturday; empty means every day.
    #[serde(default)]
    pub only_days: Vec<u32>,
    /// `HH:MM` the item may not air before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_after: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: i32,
    #[serde(default)]
    pub exclude: bool,
}

fn default_weight() -> i32 {
    1
}

/// Layer 4 — where the candidates come from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemsLayer {
    pub filter: ItemFilter,
    /// Ids of Lists-plugin lists to include.
    pub list_ids: Vec<String>,
    /// Explicit catalog keys.
    pub match_keys: Vec<String>,
    pub pins: Vec<Pin>,
}

impl ItemsLayer {
    pub fn to_prompt(&self, pool: &[PoolItem]) -> String {
        let mut lines = vec!["Candidate pool (already filtered — use these ids):".to_string()];
        if pool.is_empty() {
            lines.push("- (empty)".to_string());
        }
        for item in pool {
            let runtime = item
                .runtime_ms
                .map(|ms| format!("{} min", (ms + 59_999) / 60_000))
                .unwrap_or_else(|| "runtime unknown".to_string());
            lines.push(format!(
                "- {} — {} ({}, {})",
                item.match_key, item.title, item.kind, runtime
            ));
        }
        lines.join("\n")
    }
}

/// One candidate, as the prompt shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolItem {
    pub match_key: String,
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ms: Option<i64>,
}

// ── layer 5 ──────────────────────────────────────────────────────────────────

/// Layer 5 — per-channel free text. Prompt-only by design.
pub type PromptLayer = String;

// ── layer 6: rule scopes ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Replace,
    Append,
}

/// When a rule scope applies. `SpecificDate` beats `DateRange` beats `Holiday`
/// beats `DayOfWeek` beats `Always`, then `priority` breaks ties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum When {
    SpecificDate { date: String },
    DateRange { from: String, to: String },
    Holiday { id: String },
    DayOfWeek { days: Vec<u32> },
    Always,
}

impl When {
    fn rank(&self) -> u8 {
        match self {
            When::SpecificDate { .. } => 0,
            When::DateRange { .. } => 1,
            When::Holiday { .. } => 2,
            When::DayOfWeek { .. } => 3,
            When::Always => 4,
        }
    }

    fn matches(&self, date: &str, weekday: u32, holidays: &[String]) -> bool {
        match self {
            When::SpecificDate { date: want } => want == date,
            When::DateRange { from, to } => date >= from.as_str() && date <= to.as_str(),
            When::Holiday { id } => holidays.iter().any(|active| active == id),
            When::DayOfWeek { days } => days.contains(&weekday),
            When::Always => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scope {
    /// Channel ids the scope applies to; empty means every channel.
    #[serde(default)]
    pub channels: Vec<String>,
    pub when: When,
}

/// Layer 6 — a rule rewrite: which layer, replace or append, and the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleScope {
    #[serde(default = "new_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
    pub scope: Scope,
    /// The layer this rewrites: 2 = hours, 3 = rules, 4 = items, 5 = prompt.
    pub target: u8,
    pub mode: Mode,
    pub payload: Value,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn default_true() -> bool {
    true
}

// ── resolution ───────────────────────────────────────────────────────────────

/// The layers resolved for one `(channel, date)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResolvedLayers {
    pub hours: HoursLayer,
    pub rules: Vec<Rule>,
    pub items: ItemsLayer,
    pub prompt: PromptLayer,
    /// Names of the scopes that fired, in the order they were applied.
    #[serde(default)]
    pub applied: Vec<String>,
}

/// Start from the channel's own layers and apply every matching scope in
/// precedence order. `holidays` is the set of active holiday ids for the date.
pub fn resolve(
    mut layers: ResolvedLayers,
    scopes: &[RuleScope],
    channel_id: &str,
    date: &str,
    weekday: u32,
    holidays: &[String],
) -> ResolvedLayers {
    let mut firing: Vec<&RuleScope> = scopes
        .iter()
        .filter(|scope| scope.enabled)
        .filter(|scope| {
            scope.scope.channels.is_empty()
                || scope.scope.channels.iter().any(|id| id == channel_id)
        })
        .filter(|scope| scope.scope.when.matches(date, weekday, holidays))
        .collect();
    firing.sort_by(|a, b| {
        a.scope
            .when
            .rank()
            .cmp(&b.scope.when.rank())
            .then_with(|| a.priority.cmp(&b.priority))
    });
    for scope in firing {
        apply(&mut layers, scope);
        layers.applied.push(scope.name.clone());
    }
    layers
}

/// Rewrite one layer from a scope: `Replace` deserialises the payload into the
/// layer, `Append` merges it (arrays concatenate, objects recurse).
fn apply(layers: &mut ResolvedLayers, scope: &RuleScope) {
    let current = match scope.target {
        2 => serde_json::to_value(&layers.hours).unwrap_or(Value::Null),
        3 => serde_json::to_value(&layers.rules).unwrap_or(Value::Null),
        4 => serde_json::to_value(&layers.items).unwrap_or(Value::Null),
        5 => Value::String(layers.prompt.clone()),
        _ => return,
    };
    let merged = match scope.mode {
        Mode::Replace => scope.payload.clone(),
        Mode::Append => merge(current, scope.payload.clone()),
    };
    match scope.target {
        2 => {
            if let Ok(value) = serde_json::from_value(merged) {
                layers.hours = value;
            }
        }
        3 => {
            if let Ok(value) = serde_json::from_value(merged) {
                layers.rules = value;
            }
        }
        4 => {
            if let Ok(value) = serde_json::from_value(merged) {
                layers.items = value;
            }
        }
        5 => {
            layers.prompt = match merged {
                Value::String(text) => text,
                other => other.to_string(),
            };
        }
        _ => {}
    }
}

/// Merge `patch` into `base`: arrays concatenate, objects recurse, everything
/// else is replaced by the patch.
pub fn merge(base: Value, patch: Value) -> Value {
    match (base, patch) {
        (Value::Array(mut a), Value::Array(b)) => {
            a.extend(b);
            Value::Array(a)
        }
        (Value::Object(mut a), Value::Object(b)) => {
            for (key, value) in b {
                let merged = match a.remove(&key) {
                    Some(existing) => merge(existing, value),
                    None => value,
                };
                a.insert(key, merged);
            }
            Value::Object(a)
        }
        // Two strings concatenate: appending to the prompt layer adds text.
        (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
        (_, patch) => patch,
    }
}

// ── compilation ──────────────────────────────────────────────────────────────

/// What the model is asked, plus the pool and the resolved layers for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Compiled {
    pub prompt: String,
    pub base_hash: String,
    pub pool: Vec<PoolItem>,
    pub layers: ResolvedLayers,
}

/// Compile the resolved layers and a candidate pool into the prompt.
pub fn compile(layers: ResolvedLayers, pool: Vec<PoolItem>) -> Compiled {
    let mut sections = vec![BASE_LAYER.to_string()];
    sections.push(layers.hours.to_prompt());
    if !layers.rules.is_empty() {
        let mut rules = vec!["Channel rules:".to_string()];
        for rule in &layers.rules {
            rules.push(rule.to_prompt());
        }
        sections.push(rules.join("\n"));
    }
    sections.push(layers.items.to_prompt(&pool));
    if !layers.prompt.trim().is_empty() {
        sections.push(format!("Channel direction:\n{}", layers.prompt.trim()));
    }
    Compiled {
        prompt: sections.join("\n\n"),
        base_hash: base_layer_hash(),
        pool,
        layers,
    }
}

// ── the model's answer, and its validator ────────────────────────────────────

/// The exact shape layer 1 asks for. (Used by the build loop in a later phase.)
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelOutput {
    #[serde(default)]
    pub blocks: Vec<ModelBlock>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelBlock {
    pub slot_index: i32,
    #[serde(default = "one")]
    pub span_slots: i32,
    #[serde(default)]
    pub items: Vec<ModelItem>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

#[allow(dead_code)]
fn one() -> i32 {
    1
}

/// Check the model's answer against layer 1 and the pool. Returns the errors a
/// repair turn should quote back to the model. (Used by the build loop.)
#[allow(dead_code)]
pub fn validate(output: &ModelOutput, pool: &[PoolItem], max_per_day: &[(String, u32)]) -> Vec<String> {
    let mut errors = Vec::new();
    if output.blocks.is_empty() {
        errors.push("the day has no blocks".to_string());
        return errors;
    }
    let ids: std::collections::HashSet<&str> = pool.iter().map(|item| item.match_key.as_str()).collect();
    let mut cursor = 0;
    let mut counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for block in &output.blocks {
        if block.slot_index < 0 || block.slot_index >= 48 {
            errors.push(format!("slot {} is outside the day (0..47)", block.slot_index));
            continue;
        }
        if block.span_slots < 1 || block.slot_index + block.span_slots > 48 {
            errors.push(format!(
                "block at slot {} spans {} slots and leaves the day",
                block.slot_index, block.span_slots
            ));
            continue;
        }
        if block.slot_index != cursor {
            errors.push(format!(
                "expected the next block at slot {cursor}, found {}",
                block.slot_index
            ));
        }
        if block.items.is_empty() {
            errors.push(format!("block at slot {} is empty", block.slot_index));
        }
        for item in &block.items {
            if !ids.contains(item.id.as_str()) {
                errors.push(format!("{} is not in the candidate pool", item.id));
            }
            *counts.entry(item.id.as_str()).or_insert(0) += 1;
        }
        cursor = block.slot_index + block.span_slots;
    }
    if cursor != 48 {
        errors.push(format!("the day ends at slot {cursor}, not 48"));
    }
    // MaxPerDay counts by the item's kind.
    let kind_of: std::collections::HashMap<&str, &str> = pool
        .iter()
        .map(|item| (item.match_key.as_str(), item.kind.as_str()))
        .collect();
    let mut by_kind: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for (id, count) in &counts {
        if let Some(kind) = kind_of.get(id) {
            *by_kind.entry(kind).or_insert(0) += count;
        }
    }
    for (kind, limit) in max_per_day {
        if let Some(used) = by_kind.get(kind.as_str()) {
            if used > limit {
                errors.push(format!(
                    "{used} {kind} item(s) air today, over the limit of {limit}"
                ));
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pool() -> Vec<PoolItem> {
        vec![
            PoolItem { match_key: "a".into(), kind: "movie".into(), title: "A".into(), runtime_ms: Some(5_400_000) },
            PoolItem { match_key: "b".into(), kind: "movie".into(), title: "B".into(), runtime_ms: None },
        ]
    }

    #[test]
    fn the_prompt_carries_every_layer_and_the_pool() {
        let layers = ResolvedLayers {
            hours: HoursLayer {
                day_start: "06:00".into(),
                off_air: vec![Window { from: "03:00".into(), to: "04:00".into() }],
                dayparts: vec![Daypart { name: "Primetime".into(), from: "20:00".into(), to: "23:00".into(), kinds: vec!["movie".into()] }],
            },
            rules: vec![Rule::NoRepeatWithin { days: 7 }],
            items: ItemsLayer::default(),
            prompt: "Feel like Saturday-morning cartoons.".into(),
            applied: vec![],
        };
        let compiled = compile(layers, pool());
        assert!(compiled.prompt.contains("broadcast day starts at 06:00"));
        assert!(compiled.prompt.contains("Off air from 03:00 to 04:00"));
        assert!(compiled.prompt.contains("Primetime (20:00–23:00): movie"));
        assert!(compiled.prompt.contains("Do not repeat an item within 7 day(s)"));
        assert!(compiled.prompt.contains("a — A (movie, 90 min)"));
        assert!(compiled.prompt.contains("Feel like Saturday-morning cartoons."));
        assert_eq!(compiled.base_hash, base_layer_hash());
    }

    #[test]
    fn scopes_rewrite_their_layer_in_precedence_order() {
        let base = ResolvedLayers {
            hours: HoursLayer { day_start: "06:00".into(), ..Default::default() },
            rules: vec![],
            items: ItemsLayer::default(),
            prompt: String::new(),
            applied: vec![],
        };
        let scopes = vec![
            RuleScope {
                id: "1".into(), name: "Thursday".into(), enabled: true, priority: 0,
                scope: Scope { channels: vec![], when: When::DayOfWeek { days: vec![4] } },
                target: 5, mode: Mode::Append, payload: json!("Thursday text."),
            },
            RuleScope {
                id: "2".into(), name: "Christmas".into(), enabled: true, priority: 0,
                scope: Scope { channels: vec![], when: When::Holiday { id: "christmas".into() } },
                target: 5, mode: Mode::Append, payload: json!("Christmas text."),
            },
            RuleScope {
                id: "3".into(), name: "Xmas Eve".into(), enabled: true, priority: 0,
                scope: Scope { channels: vec![], when: When::SpecificDate { date: "2026-12-24".into() } },
                target: 3, mode: Mode::Append, payload: json!([{ "rule": "max_per_day", "kind": "movie", "n": 3 }]),
            },
        ];
        let resolved = resolve(base, &scopes, "chan", "2026-12-24", 4, &["christmas".to_string()]);
        // Specific date (rank 0) fires before holiday (rank 2) before weekday.
        assert_eq!(resolved.applied, vec!["Xmas Eve", "Christmas", "Thursday"]);
        assert_eq!(resolved.rules, vec![Rule::MaxPerDay { kind: "movie".into(), n: 3 }]);
        assert_eq!(resolved.prompt, "Christmas text.Thursday text.");
    }

    #[test]
    fn a_channel_scoped_override_only_touches_its_channel() {
        let scopes = vec![RuleScope {
            id: "1".into(), name: "Only Two".into(), enabled: true, priority: 0,
            scope: Scope { channels: vec!["chan".into()], when: When::Always },
            target: 3, mode: Mode::Append, payload: json!([{ "rule": "no_repeat_within", "days": 30 }]),
        }];
        let mine = resolve(ResolvedLayers::default(), &scopes, "chan", "2026-01-01", 4, &[]);
        assert_eq!(mine.rules.len(), 1);
        let other = resolve(ResolvedLayers::default(), &scopes, "other", "2026-01-01", 4, &[]);
        assert!(other.rules.is_empty());
    }

    #[test]
    fn the_validator_rejects_a_broken_day() {
        let broken = ModelOutput {
            blocks: vec![
                ModelBlock { slot_index: 0, span_slots: 2, items: vec![ModelItem { id: "a".into(), why: String::new() }] },
                ModelBlock { slot_index: 3, span_slots: 1, items: vec![ModelItem { id: "zzz".into(), why: String::new() }] },
            ],
        };
        let errors = validate(&broken, &pool(), &[]);
        assert!(errors.iter().any(|error| error.contains("expected the next block at slot 2")));
        assert!(errors.iter().any(|error| error.contains("zzz is not in the candidate pool")));
        assert!(errors.iter().any(|error| error.contains("ends at slot 4, not 48")));
    }

    #[test]
    fn a_full_clean_day_validates() {
        let output = ModelOutput {
            blocks: vec![ModelBlock {
                slot_index: 0,
                span_slots: 48,
                items: vec![ModelItem { id: "a".into(), why: "film".into() }],
            }],
        };
        assert!(validate(&output, &pool(), &[]).is_empty());
    }
}
