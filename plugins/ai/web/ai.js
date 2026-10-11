let aiProviders = [];
// The AI feature is a plugin now; the shell talks to it at its own route
// namespace instead of a core `/api/ai`.
const AI_API = "/api/plugins/com.channelflow.ai";
let aiNextPriority = 1;
let aiDefaults = { base_url: "", chat_model: "", tts_model: "", voice: "", ollama_base_url: "http://localhost:11434/v1" };
let aiActiveId = null; // null is the "New provider" tab
let aiKeySet = false;

function setAiNote(message, bad) {
  els.aiNote.textContent = message || "";
  els.aiNote.className = bad ? "hint bad" : "hint";
}

function renderAiKeyState() {
  els.aiKeyClear.hidden = !aiKeySet;
  els.aiKeyHint.textContent = aiKeySet
    ? "A key is saved. Leave this blank to keep it, or type a new one to replace it."
    : "No key saved. Leave it blank for a server that needs none.";
  els.aiApiKey.placeholder = aiKeySet ? "•••••••• saved" : "sk-…";
}

// The tab strip: "New provider" first, then the saved providers in the order
// the app will try them. A provider is addressed by its id, so renaming one
// does not move it to a different tab.
function renderAiTabs() {
  els.aiTabs.replaceChildren();
  const tabs = [{ id: null, label: "New provider" }].concat(
    aiProviders.map((provider) => ({ id: provider.id, label: provider.name }))
  );
  for (const tab of tabs) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "inner-tab";
    button.textContent = tab.label;
    button.setAttribute("role", "tab");
    const active = tab.id === aiActiveId;
    button.classList.toggle("active", active);
    button.setAttribute("aria-selected", active ? "true" : "false");
    button.addEventListener("click", () => selectAiTab(tab.id));
    els.aiTabs.appendChild(button);
  }
}

// The running order shown under the form. It is the same list the tabs use, so
// what the page shows is what the app does.
function renderAiFailover() {
  els.aiFailover.replaceChildren();
  if (!aiProviders.length) {
    const item = document.createElement("li");
    item.className = "empty";
    item.textContent = "No providers yet — add one above.";
    els.aiFailover.appendChild(item);
    return;
  }
  for (const provider of aiProviders) {
    const item = document.createElement("li");
    item.className = "ai-provider-row" + (provider.enabled === false ? " ai-provider-off" : "");
    const info = document.createElement("span");
    info.className = "ai-provider-info";
    info.textContent = `${provider.priority} · ${provider.name} — ${provider.base_url}`;
    const sw = document.createElement("label");
    sw.className = "switch";
    sw.title = provider.enabled === false ? "Provider is disabled" : "Provider is enabled";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = provider.enabled !== false;
    input.dataset.providerId = provider.id;
    input.addEventListener("change", () => toggleAiProvider(provider, input));
    const track = document.createElement("span");
    track.className = "track";
    const thumb = document.createElement("span");
    thumb.className = "thumb";
    sw.appendChild(input);
    sw.appendChild(track);
    sw.appendChild(thumb);
    item.appendChild(info);
    item.appendChild(sw);
    els.aiFailover.appendChild(item);
  }
}

// Flip a provider's enabled switch through the normal update (keep-everything
// semantics), then reload so the failover order and tabs reflect it.
async function toggleAiProvider(provider, input) {
  input.disabled = true;
  try {
    await request(`${AI_API}/providers/${encodeURIComponent(provider.id)}`, {
      method: "PUT",
      body: JSON.stringify({ enabled: !(provider.enabled !== false) }),
    });
    const updated = await request(AI_API);
    aiProviders = (updated && updated.providers) || [];
    renderAiFailover();
    renderAiTabs();
    setAiNote(provider.enabled !== false ? `${provider.name} disabled.` : `${provider.name} enabled.`, false);
  } catch (error) {
    setAiNote(error.message, true);
    input.checked = provider.enabled !== false;
  } finally {
    input.disabled = false;
  }
}

// Point the one form at a provider, or at a blank new one.
function fillAiForm(provider) {
  if (provider) {
    els.aiName.value = provider.name;
    els.aiEnabled.checked = provider.enabled !== false;
    els.aiPriority.value = String(provider.priority);
    els.aiBaseUrl.value = provider.base_url;
    els.aiChatModel.value = provider.chat_model;
    els.aiTtsModel.value = provider.tts_model;
    els.aiVoice.value = provider.voice;
    els.aiSchedulingOnly.checked = !!provider.scheduling_only;
    els.aiTtsOnly.checked = !!provider.tts_only;
    els.aiOllama.checked = !!provider.ollama;
    aiKeySet = provider.api_key_set;
    els.aiSave.textContent = "Save provider";
    els.aiDelete.hidden = false;
  } else {
    els.aiName.value = "";
    els.aiEnabled.checked = true;
    els.aiPriority.value = String(aiNextPriority);
    els.aiBaseUrl.value = aiDefaults.base_url;
    els.aiChatModel.value = aiDefaults.chat_model;
    els.aiTtsModel.value = aiDefaults.tts_model;
    els.aiVoice.value = aiDefaults.voice;
    els.aiSchedulingOnly.checked = false;
    els.aiTtsOnly.checked = false;
    els.aiOllama.checked = false;
    aiKeySet = false;
    els.aiSave.textContent = "Add provider";
    els.aiDelete.hidden = true;
  }
  syncAiOllama();
  syncAiModelFields();
  els.aiApiKey.value = "";
  els.aiApiKey.type = "password";
  els.aiKeyReveal.textContent = "Show";
  renderAiKeyState();
  els.aiTestResult.hidden = true;
}

// Ollama: no API key, URL defaults to the local gateway.
function syncAiOllama() {
  if (!els.aiOllama) return;
  const isOllama = els.aiOllama.checked;
  if (isOllama) {
    if (!els.aiBaseUrl.value || els.aiBaseUrl.value === aiDefaults.base_url) {
      els.aiBaseUrl.value = aiDefaults.ollama_base_url || "http://localhost:11434/v1";
    }
  }
  if (els.aiApiKey) {
    els.aiApiKey.disabled = isOllama;
    if (isOllama) els.aiApiKey.value = "";
  }
  renderAiKeyState();
}

function fillModelList(id, models) {
  const datalist = $(id);
  if (!datalist) return;
  datalist.textContent = "";
  (models || []).forEach((model) => {
    const option = document.createElement("option");
    option.value = model;
    datalist.appendChild(option);
  });
}

// A scheduling-only provider takes no TTS model or voice; a TTS-only provider
// takes no chat model. Disable (and blank) the fields it won't use.
function syncAiModelFields() {
  if (!els.aiSchedulingOnly || !els.aiTtsOnly) return;
  const schedulingOnly = els.aiSchedulingOnly.checked;
  const ttsOnly = els.aiTtsOnly.checked;
  [els.aiTtsModel, els.aiVoice].forEach((el) => {
    if (!el) return;
    el.disabled = schedulingOnly;
    if (schedulingOnly) el.value = "";
  });
  if (els.aiChatModel) {
    els.aiChatModel.disabled = ttsOnly;
    if (ttsOnly) els.aiChatModel.value = "";
  }
}

// Grab the models the provider lists (Ollama's /v1/models included) and offer
// them in the Chat/TTS dropdowns, plus the common voice ids.
async function fetchProviderModels() {
  const button = $("ai-fetch-models");
  if (button) button.disabled = true;
  setAiNote("Fetching models…");
  try {
    const data = await request(`${AI_API}/models`, {
      method: "POST",
      body: JSON.stringify({
        base_url: els.aiBaseUrl.value,
        api_key: els.aiApiKey.value.trim(),
        ollama: !!(els.aiOllama && els.aiOllama.checked),
      }),
    });
    if (!data.ok) throw new Error(data.error || "Could not fetch models.");
    fillModelList("ai-chat-models", data.models);
    fillModelList("ai-tts-models", data.models);
    fillModelList("ai-voices", data.voices || []);
    setAiNote(`${data.models.length} model(s) fetched from ${data.base_url}.`);
  } catch (error) {
    setAiNote(error.message, true);
  } finally {
    if (button) button.disabled = false;
  }
}

function selectAiTab(id) {
  if (id !== null && !aiProviders.some((provider) => provider.id === id)) {
    id = null;
  }
  aiActiveId = id;
  fillAiForm(id === null ? null : aiProviders.find((provider) => provider.id === id));
  renderAiTabs();
  els.aiFailoverResult.hidden = true;
  setAiNote("");
}

async function loadAi(selectId) {
  try {
    const data = await request(AI_API);
    aiProviders = data.providers;
    aiNextPriority = data.next_priority;
    aiDefaults = data.defaults;
    const wanted = selectId !== undefined ? selectId : aiActiveId;
    renderAiFailover();
    selectAiTab(wanted);
  } catch (error) {
    setAiNote(error.message || "Could not load AI settings.", true);
  }
}

// The name, URL, models and a new key are always sent — clearing one is a
// validation error worth showing, not a silent fall-back. Priority is left out
// when the field is blank, which keeps the stored number on an edit; creating
// a provider needs it, so `saveAi` checks first. The key is only sent when the
// field holds something, so a blank field keeps the saved key.
function aiBody() {
  const body = {
    name: els.aiName.value,
    enabled: els.aiEnabled.checked,
    base_url: els.aiBaseUrl.value,
    chat_model: els.aiChatModel.value,
    tts_model: els.aiTtsModel.value,
    voice: els.aiVoice.value,
    scheduling_only: els.aiSchedulingOnly.checked,
    tts_only: !!(els.aiTtsOnly && els.aiTtsOnly.checked),
    ollama: !!(els.aiOllama && els.aiOllama.checked),
  };
  const priority = els.aiPriority.value.trim();
  if (priority !== "") body.priority = Number.parseInt(priority, 10);
  const key = els.aiApiKey.value.trim();
  if (key !== "") body.api_key = key;
  return body;
}

function newPriorityMissing() {
  return aiActiveId === null && els.aiPriority.value.trim() === "";
}

async function saveAi() {
  if (newPriorityMissing()) {
    setAiNote("Give the provider a priority — a whole number, lowest is tried first.", true);
    return;
  }
  try {
    let saved;
    let message;
    if (aiActiveId === null) {
      saved = await request(`${AI_API}/providers`, {
        method: "POST",
        body: JSON.stringify(aiBody()),
      });
      message = `Added ${saved.name}.`;
    } else {
      saved = await request(`${AI_API}/providers/${encodeURIComponent(aiActiveId)}`, {
        method: "PUT",
        body: JSON.stringify(aiBody()),
      });
      message = `Saved ${saved.name}.`;
    }
    await loadAi(saved.id);
    setAiNote(message);
  } catch (error) {
    setAiNote(error.message || "Could not save the provider.", true);
  }
}

async function deleteAi() {
  const provider = aiProviders.find((entry) => entry.id === aiActiveId);
  if (!provider) return;
  if (!window.confirm(`Delete the "${provider.name}" provider?`)) return;
  try {
    await request(`${AI_API}/providers/${encodeURIComponent(provider.id)}`, { method: "DELETE" });
    await loadAi(null);
    setAiNote(`Deleted ${provider.name}.`);
  } catch (error) {
    setAiNote(error.message || "Could not delete the provider.", true);
  }
}

// Tests what is on the page rather than what is saved, so a new key, model or
// priority can be checked before it is written. It makes the real requests — a
// model listing, a chat reply and one spoken phrase — and stores nothing.
async function testAi() {
  if (newPriorityMissing()) {
    setAiNote("Give the provider a priority before testing it.", true);
    return;
  }
  els.aiTest.disabled = true;
  els.aiTest.textContent = "Testing…";
  els.aiTestResult.hidden = true;
  setAiNote("");
  try {
    const path =
      aiActiveId === null
        ? `${AI_API}/test`
        : `${AI_API}/providers/${encodeURIComponent(aiActiveId)}/test`;
    renderAiTest(await request(path, { method: "POST", body: JSON.stringify(aiBody()) }));
  } catch (error) {
    setAiNote(error.message || "Could not run the test.", true);
  } finally {
    els.aiTest.disabled = false;
    els.aiTest.textContent = "Test AI";
  }
}

function renderAiTest(report) {
  const head = report.ok
    ? `Connected. Spoke ${formatBytes(report.bytes)} in ${report.elapsed_ms} ms.`
    : "The test did not pass.";
  els.aiTestResult.className = "test-result " + (report.ok ? "ok" : "bad");
  els.aiTestResult.innerHTML =
    `<p class="test-head">${escapeHtml(head)}</p>` +
    report.probes
      .map(
        (probe) =>
          `<div class="test-probe"><span class="test-name">${escapeHtml(probe.name)}</span>` +
          `<span class="test-detail ${probe.ok ? "ok" : "bad"}">${escapeHtml(probe.detail)}</span></div>`
      )
      .join("");
  els.aiTestResult.hidden = false;
}

// Walk the providers in priority order and stop at the first that answers, so
// the page shows which endpoint the app would actually use — and which ones
// are never reached because a higher-priority one works.
async function testAiAll() {
  els.aiTestAll.disabled = true;
  els.aiTestAll.textContent = "Testing…";
  els.aiFailoverResult.hidden = true;
  try {
    renderFailover(await request(`${AI_API}/test-all`, { method: "POST" }));
  } catch (error) {
    els.aiFailoverResult.className = "test-result bad";
    els.aiFailoverResult.innerHTML = `<p class="test-head">${escapeHtml(
      error.message || "Could not run the failover test."
    )}</p>`;
    els.aiFailoverResult.hidden = false;
  } finally {
    els.aiTestAll.disabled = false;
    els.aiTestAll.textContent = "Test failover";
  }
}

function renderFailover(report) {
  const head = report.ok
    ? `Connected — "${report.chosen}" answers first.`
    : "No provider answered.";
  els.aiFailoverResult.className = "test-result " + (report.ok ? "ok" : "bad");
  els.aiFailoverResult.innerHTML =
    `<p class="test-head">${escapeHtml(head)}</p>` +
    report.attempts
      .map((attempt) => {
        const suffix = report.chosen === attempt.name && attempt.ok ? " (used first)" : "";
        return (
          `<div class="test-probe"><span class="test-name">${attempt.priority}</span>` +
          `<span class="test-detail ${attempt.ok ? "ok" : "bad"}">${escapeHtml(
            attempt.name
          )} — ${escapeHtml(attempt.detail)}${suffix}</span></div>`
        );
      })
      .join("");
  els.aiFailoverResult.hidden = false;
}

function formatBytes(bytes) {
  if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

els.aiSave.addEventListener("click", saveAi);
const aiTtsOnly = $("ai-tts-only");
if (aiTtsOnly) {
  aiTtsOnly.addEventListener("change", () => {
    if (aiTtsOnly.checked && $("ai-scheduling-only")) $("ai-scheduling-only").checked = false;
    syncAiModelFields();
  });
}
const aiSchedulingOnlyCb = $("ai-scheduling-only");
if (aiSchedulingOnlyCb) {
  aiSchedulingOnlyCb.addEventListener("change", () => {
    if (aiSchedulingOnlyCb.checked && aiTtsOnly) aiTtsOnly.checked = false;
    syncAiModelFields();
  });
}
els.aiOllama.addEventListener("change", syncAiOllama);
els.aiFetchModels.addEventListener("click", fetchProviderModels);
els.aiTest.addEventListener("click", testAi);
els.aiDelete.addEventListener("click", deleteAi);
els.aiTestAll.addEventListener("click", testAiAll);
els.aiKeyClear.addEventListener("click", async () => {
  if (aiActiveId === null) return;
  try {
    await request(`${AI_API}/providers/${encodeURIComponent(aiActiveId)}`, {
      method: "PUT",
      body: JSON.stringify({ api_key: "" }),
    });
    aiKeySet = false;
    renderAiKeyState();
    setAiNote("Saved key removed.");
  } catch (error) {
    setAiNote(error.message || "Could not remove the key.", true);
  }
});
els.aiKeyReveal.addEventListener("click", () => {
  const hidden = els.aiApiKey.type === "password";
  els.aiApiKey.type = hidden ? "text" : "password";
  els.aiKeyReveal.textContent = hidden ? "Hide" : "Show";
});

// --- Transcode -------------------------------------------------------------
// The Transcode feature is the ErsatzTV plugin; the page calls its routes.

CF.define("ai", { onShow: () => loadAi() });
