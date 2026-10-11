//! The "rewrite as one news show" pass.
//!
//! With the option on, the day's headlines go to an AI chat provider with a
//! news-writer brief, so the bulletin reads as one flowing broadcast rather
//! than a pile of articles stitched together. The provider comes from the AI
//! page (the core hands the lowest-priority usable chat endpoint to the
//! plugin); any failure leaves the caller with the original copy, so a missing
//! or unreachable model never stops the newscast.

use std::sync::Arc;

use channelflow_plugin_api::core::LlmProvider;
use serde_json::{json, Value};

use crate::settings::NewsSettings;
use crate::NewsState;

/// The brief the writer is given. It must keep the facts, keep the order and
/// the story count, but write one show.
const SYSTEM: &str = "You are the writer for a television newscast. You take \
raw RSS headlines and rewrite them into ONE flowing broadcast, the way a TV \
anchor reads the news. Rules: keep every fact exactly; invent nothing — no new \
names, numbers, places or claims; drop opinion, hype and speculation; write \
short, plain, spoken sentences and vary how each one opens; use natural \
transitions (\"Meanwhile…\", \"In Iowa…\") so the items flow as one show rather \
than a list of unrelated articles; do not just copy the source text — tighten \
it into broadcast copy.";

/// The rewrite walks an 8B-class model through a whole show, and the first
/// call of the day also loads the model, so give it far longer than the
/// request client's default (feeds use 20s; a rewrite can take minutes).
const AI_TIMEOUT_SECONDS: u64 = 180;

/// The most stories to hand the writer at once, so a long feed cannot blow up
/// the prompt.
const MAX_STORIES: usize = 30;

/// Rewrite `stories` into one newscast: one output per input, same order, same
/// `feed`/`image`, new `title`/`summary`. Returns the originals unchanged (not
/// an error) when there is nothing to gain or no provider is set, so the
/// caller can always just use the result.
pub async fn rewrite_show(
    state: &Arc<NewsState>,
    settings: &NewsSettings,
    stories: &[Value],
) -> Vec<Value> {
    if stories.len() < 2 {
        return stories.to_vec();
    }
    let selected = &stories[..stories.len().min(MAX_STORIES)];
    match run(state, settings, selected).await {
        Ok(rewritten) => rewritten,
        Err(error) => {
            state
                .logger
                .warn(&format!("news AI rewrite skipped: {error}"));
            stories.to_vec()
        }
    }
}

/// The fallible core: get a provider, ask it, parse the reply.
async fn run(
    state: &Arc<NewsState>,
    settings: &NewsSettings,
    stories: &[Value],
) -> Result<Vec<Value>, String> {
    let provider = state
        .core
        .llm_provider()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "no AI chat provider is configured (add one on the AI page)".to_string())?;
    let show_name = crate::api::resolve_show_name(&settings.header);
    let raw = chat(state, &provider, &show_name, stories).await?;
    parse(&raw, stories)
}

/// Ask the provider's OpenAI-compatible `/chat/completions` for the show.
async fn chat(
    state: &Arc<NewsState>,
    provider: &LlmProvider,
    show_name: &str,
    stories: &[Value],
) -> Result<String, String> {
    let messages = json!([
        {
            "role": "system",
            "content": format!("{SYSTEM} The show is called {show_name}."),
        },
        { "role": "user", "content": user_prompt(stories) },
    ]);
    let body = json!({
        "model": provider.chat_model,
        "messages": messages,
        "temperature": 0.4,
    });
    let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));
    let mut request = state
        .http
        .post(&url)
        .timeout(std::time::Duration::from_secs(AI_TIMEOUT_SECONDS))
        .json(&body);
    if !provider.api_key.trim().is_empty() {
        request = request.bearer_auth(provider.api_key.trim());
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("could not reach the AI provider: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|_| "could not read the AI reply".to_string())?;
    if !status.is_success() {
        return Err(format!("the AI provider answered {status}: {}", head(&text)));
    }
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|_| "the AI provider returned invalid JSON".to_string())?;
    parsed["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the AI reply carried no text".to_string())
}

/// The writer's instructions: the stories, the shape wanted, and the rules.
fn user_prompt(stories: &[Value]) -> String {
    let items: Vec<Value> = stories
        .iter()
        .enumerate()
        .map(|(index, story)| {
            json!({
                "position": index + 1,
                "title": story.get("title").and_then(Value::as_str).unwrap_or(""),
                "summary": story.get("summary").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();
    let count = items.len();
    let rendered = serde_json::to_string_pretty(&items).unwrap_or_default();
    format!(
        "Here are {count} stories from today's feeds, in broadcast order. Rewrite them as one \
         newscast.\n\
         Return ONLY a JSON array, with no prose and no code fences, holding exactly {count} \
         objects in the same order. Each object has:\n\
         - \"title\": a short broadcast headline (about 12 words at most)\n\
         - \"summary\": one or two spoken sentences for the anchor, keeping to the facts\n\
         Stories:\n{rendered}"
    )
}

/// Pull the JSON array out of the model's reply (it may be wrapped in prose or
/// code fences), then map each object back onto its story.
fn parse(raw: &str, stories: &[Value]) -> Result<Vec<Value>, String> {
    let text = array_slice(raw).ok_or_else(|| "the AI reply held no JSON array".to_string())?;
    let items: Vec<Value> = serde_json::from_str(text)
        .map_err(|error| format!("the AI reply was not valid JSON: {error}"))?;
    if items.len() != stories.len() {
        return Err(format!(
            "the AI returned {} stories for {}",
            items.len(),
            stories.len()
        ));
    }
    let mut out = Vec::with_capacity(stories.len());
    for (story, item) in stories.iter().zip(items.iter()) {
        let title = item.get("title").and_then(Value::as_str).unwrap_or("").trim();
        let summary = item.get("summary").and_then(Value::as_str).unwrap_or("").trim();
        if title.is_empty() || summary.is_empty() {
            return Err("the AI reply left a storyline empty".to_string());
        }
        // Keep the story's own feed and image; only the copy is rewritten.
        let mut rewritten = story.clone();
        if let Some(object) = rewritten.as_object_mut() {
            object.insert("title".to_string(), Value::String(title.to_string()));
            object.insert("summary".to_string(), Value::String(summary.to_string()));
        }
        out.push(rewritten);
    }
    Ok(out)
}

/// The first `[…]` run in `text` — the reply may carry prose or code fences
/// around the array.
fn array_slice(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end > start {
        Some(&text[start..=end])
    } else {
        None
    }
}

/// A one-line, length-capped excerpt of an error body for the log.
fn head(text: &str) -> String {
    let flat = text.replace('\n', " ");
    let mut out: String = flat.trim().chars().take(140).collect();
    if flat.trim().chars().count() > 140 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn story(title: &str, summary: &str) -> Value {
        json!({
            "title": title,
            "summary": summary,
            "feed": "KCCI",
            "image": "https://img.example/one.png"
        })
    }

    #[test]
    fn maps_the_models_lines_back_onto_the_stories() {
        let stories = vec![story("Old one", "old body"), story("Old two", "old body two")];
        let reply = "Here is the newscast:\n```json\n[\n  \
            {\"title\": \"Storm rolls in\", \"summary\": \"Storms arrive tonight.\"},\n  \
            {\"title\": \"Council votes\", \"summary\": \"The council voted 5-2.\"}\n]\n```";
        let out = parse(reply, &stories).expect("parses");
        assert_eq!(out[0]["title"], "Storm rolls in");
        assert_eq!(out[0]["summary"], "Storms arrive tonight.");
        assert_eq!(out[1]["title"], "Council votes");
        // The feed and image carry over from the original untouched.
        assert_eq!(out[0]["feed"], "KCCI");
        assert_eq!(out[1]["image"], "https://img.example/one.png");
    }

    #[test]
    fn trims_the_rewritten_lines() {
        let stories = vec![story("A", "a"), story("B", "b")];
        let reply = r#"[{"title":"  Storm  ","summary":"  It rains.  "},{"title":"Two","summary":"More."}]"#;
        let out = parse(reply, &stories).expect("parses");
        assert_eq!(out[0]["title"], "Storm");
        assert_eq!(out[0]["summary"], "It rains.");
    }

    #[test]
    fn rejects_a_reply_with_the_wrong_shape() {
        let stories = vec![story("A", "a"), story("B", "b")];
        // Wrong count.
        assert!(parse(r#"[{"title":"x","summary":"y"}]"#, &stories).is_err());
        // Empty text in a line.
        assert!(parse(
            r#"[{"title":"","summary":"y"},{"title":"x","summary":"z"}]"#,
            &stories
        )
        .is_err());
        // No array at all.
        assert!(parse("I could not do that.", &stories).is_err());
        // Truncated JSON.
        assert!(parse(r#"[{"title":"x","#, &stories).is_err());
    }

    #[test]
    fn finds_the_json_array_inside_noise() {
        assert_eq!(
            array_slice("blah\n[1,2]\ntrailing"),
            Some("[1,2]")
        );
        assert_eq!(array_slice("no brackets here"), None);
        assert_eq!(array_slice("] before [ is nothing"), None);
    }

    #[test]
    fn head_flattens_and_caps() {
        let long = "x".repeat(200);
        let out = head(&long);
        assert_eq!(out.chars().count(), 141, "140 characters plus the ellipsis");
        assert!(out.ends_with('…'));
        assert_eq!(head("a\nb"), "a b");
        assert_eq!(head("short"), "short");
    }

    #[test]
    fn the_prompt_lists_every_story_in_order() {
        let stories = vec![story("First", "one"), story("Second", "two")];
        let prompt = user_prompt(&stories);
        assert!(prompt.contains("2 stories"));
        assert!(prompt.contains("exactly 2 objects"));
        let first = prompt.find("First").expect("first present");
        let second = prompt.find("Second").expect("second present");
        assert!(first < second, "stories keep their broadcast order");
    }
}
