const NEWS_API = "/api/plugins/com.channelflow.news";
const NEWS_VOICE_LABELS = {
  "en-US": "English (US)",
  "en-GB": "English (UK)",
  es: "Spanish",
  fr: "French",
  de: "German",
  it: "Italian",
  pt: "Portuguese",
  ja: "Japanese",
  ko: "Korean",
  "zh-CN": "Chinese",
};

function fillNewsSelect(id, values, labels, selected) {
  const select = $(id);
  if (!select || !values) return;
  select.textContent = "";
  values.forEach((value) => {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = (labels && labels[value]) || String(value);
    select.appendChild(option);
  });
  if (selected) select.value = selected;
}

function setNewsCheck(id, value) {
  const el = $(id);
  if (el) el.checked = value !== false;
}

function collectNewsSettings() {
  return {
    header: $("news-header").value.trim(),
    article_count: Number($("news-count").value) || 8,
    refresh_minutes: Number($("news-refresh").value) || 10,
    tts_voice: $("news-voice").value,
    anchor_intro: $("news-intro").value.trim(),
    anchor_outro: $("news-outro").value.trim(),
    tts_enabled: $("news-tts").checked,
    tts_engine: $("news-tts-engine").value,
    ai_rewrite: $("news-ai-rewrite").checked,
    show_header: $("news-show-header").checked,
    headlines_only: $("news-headlines-only").checked,
    no_music: $("news-no-music").checked,
    bulletin_enabled: $("news-bulletin-enabled").checked,
    minimum_new_stories: Number($("news-min-new").value) || 1,
    bulletin_times: $("news-bulletin-times").value.trim() || "6,10,14,18,20,22",
    feeds: $("news-feeds").value.split("\n").map((url) => url.trim()).filter(Boolean).map((url) => ({ url, enabled: true })),
  };
}

async function loadNews() {
  try {
    const data = await request(NEWS_API);
    const settings = data.settings || {};
    fillNewsSelect("news-voice", (data.options && data.options.tts_voices) || [], NEWS_VOICE_LABELS, settings.tts_voice);
    fillNewsSelect("news-tts-engine", (data.options && data.options.tts_engines) || [], { google: "Basic Google TTS", ai: "AI TTS" }, settings.tts_engine);
    $("news-header").value = settings.header || "";
    $("news-count").value = settings.article_count;
    $("news-refresh").value = settings.refresh_minutes;
    $("news-intro").value = settings.anchor_intro || "";
    $("news-outro").value = settings.anchor_outro || "";
    setNewsCheck("news-tts", settings.tts_enabled);
    setNewsCheck("news-ai-rewrite", settings.ai_rewrite);
    setNewsCheck("news-show-header", settings.show_header);
    setNewsCheck("news-headlines-only", settings.headlines_only);
    setNewsCheck("news-no-music", settings.no_music);
    setNewsCheck("news-bulletin-enabled", settings.bulletin_enabled);
    $("news-min-new").value = settings.minimum_new_stories;
    $("news-bulletin-times").value = settings.bulletin_times || "6,10,14,18,20,22";
    loadBulletinStatus();
    $("news-feeds").value = (settings.feeds || []).filter((feed) => feed.enabled).map((feed) => feed.url).join("\n");
    const list = $("news-preview-list");
    if (list) list.hidden = true;
    $("news-result").textContent = "";
  } catch (error) {
    $("news-result").textContent = error.message;
  }
}

async function saveNews(event) {
  if (event && event.preventDefault) event.preventDefault();
  const save = $("news-save");
  if (save) save.disabled = true;
  try {
    await request(NEWS_API, { method: "PUT", body: JSON.stringify(collectNewsSettings()) });
    if ($("news-result")) $("news-result").textContent = "Saved.";
  } catch (error) {
    if ($("news-result")) $("news-result").textContent = error.message;
  } finally {
    if (save) save.disabled = false;
  }
}

function renderNewsPreview(feeds) {
  const list = $("news-preview-list");
  if (!list) return;
  list.textContent = "";
  let total = 0;
  (feeds || []).forEach((feed) => {
    const card = document.createElement("div");
    card.className = "ms-card";
    const headEl = document.createElement("div");
    headEl.className = "ms-card-head";
    const title = document.createElement("h4");
    title.textContent = feed.url;
    headEl.appendChild(title);
    card.appendChild(headEl);
    const headlines = feed.headlines || [];
    total += headlines.length;
    if (feed.error) {
      const note = document.createElement("p");
      note.className = "ms-status bad";
      note.textContent = feed.error;
      card.appendChild(note);
    }
    if (headlines.length) {
      const ul = document.createElement("ul");
      ul.className = "failover-list";
      headlines.forEach((headline, index) => {
        const li = document.createElement("li");
        li.textContent = `${index + 1}. ${headline}`;
        ul.appendChild(li);
      });
      card.appendChild(ul);
    }
    list.appendChild(card);
  });
  list.hidden = false;
  $("news-result").textContent = total ? `${total} headline(s)` : "No headlines found.";
}

function renderNewsPreview(show) {
  const list = $("news-preview-list");
  if (!list) return;
  list.textContent = "";
  const stories = (show && show.stories) || [];
  const script = document.createElement("div");
  script.className = "ms-card";
  if (show && show.show_header) {
    const name = document.createElement("h3");
    name.className = "news-show-name";
    name.textContent = show.header || "";
    script.appendChild(name);
  }
  const intro = document.createElement("p");
  intro.className = "news-script-intro";
  intro.textContent = (show && show.intro) || "";
  script.appendChild(intro);
  if (stories.length) {
    const ol = document.createElement("ol");
    ol.className = "news-stories";
    stories.forEach((story) => {
      const li = document.createElement("li");
      const title = document.createElement("div");
      title.className = "news-story-title";
      title.textContent = story.title || "";
      li.appendChild(title);
      if (!(show && show.headlines_only) && story.summary) {
        const summary = document.createElement("div");
        summary.className = "news-story-summary";
        summary.textContent = story.summary;
        li.appendChild(summary);
      }
      if (story.feed) {
        const feed = document.createElement("div");
        feed.className = "news-story-feed";
        feed.textContent = story.feed;
        li.appendChild(feed);
      }
      ol.appendChild(li);
    });
    script.appendChild(ol);
  }
  const outro = document.createElement("p");
  outro.className = "news-script-outro";
  outro.textContent = (show && show.outro) || "";
  script.appendChild(outro);
  list.appendChild(script);
  list.hidden = false;
  $("news-result").textContent = stories.length
    ? `${stories.length} storie(s) on the newscast`
    : "No headlines found.";
}

async function previewNews() {
  const button = $("news-preview");
  if (button) button.disabled = true;
  try {
    // Save first so the preview uses the feeds the form holds.
    await request(NEWS_API, { method: "PUT", body: JSON.stringify(collectNewsSettings()) });
    const data = await request(NEWS_API + "/preview");
    renderNewsPreview(data);
  } catch (error) {
    const list = $("news-preview-list");
    if (list) {
      list.hidden = false;
      list.textContent = "";
      const note = document.createElement("p");
      note.className = "ms-status bad";
      note.textContent = error.message;
      list.appendChild(note);
    }
    $("news-result").textContent = "Could not preview.";
  } finally {
    if (button) button.disabled = false;
  }
}

async function loadBulletinStatus() {
  const note = $("news-bulletin-status");
  if (!note) return;
  try {
    const data = await request(NEWS_API + "/bulletin");
    const b = data.bulletin || {};
    if (b.last_created) {
      note.textContent = `Last video built ${b.last_run || ""} — ${b.last_encoded_count} stories.`;
    } else if (b.last_skip_reason) {
      note.textContent = `Last run skipped: ${b.last_skip_reason}`;
    } else {
      note.textContent = `Builds at hours ${b.times || "6,10,14,18,20,22"} (once a day for sure).`;
    }
  } catch (error) {
    note.textContent = "";
  }
}

async function buildBulletinNow() {
  const button = $("news-bulletin-build");
  if (button) button.disabled = true;
  const note = $("news-bulletin-status");
  if (note) note.textContent = "Building the news video…";
  try {
    // Save first so the build uses the feeds and times the form holds.
    await request(NEWS_API, { method: "PUT", body: JSON.stringify(collectNewsSettings()) });
    const data = await request(NEWS_API + "/bulletin/run", { method: "POST" });
    const outcome = data.outcome || {};
    if (outcome.created) {
      note.textContent = `Built ${outcome.path || "the video"} with ${outcome.encoded_count} stories.`;
    } else {
      note.textContent = outcome.reason || "The build did not create a video.";
    }
  } catch (error) {
    if (note) note.textContent = error.message;
  } finally {
    if (button) button.disabled = false;
  }
}

{
  const form = $("news-form");
  if (form) form.addEventListener("submit", saveNews);
  const preview = $("news-preview");
  if (preview) preview.addEventListener("click", previewNews);
  const build = $("news-bulletin-build");
  if (build) build.addEventListener("click", buildBulletinNow);
}

CF.define("news", { onShow: loadNews });
