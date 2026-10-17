// Scheduler: settings, the per-channel layers, rule scopes, preview/build, and
// the generated-day view. Everything goes through the plugin's own routes.

const SCHED = "/api/plugins/com.channelflow.scheduler";
let schedSettings = {};
let schedChannels = [];
let schedScopes = [];
let schedDays = [];
let schedChannelId = "";

async function loadScheduler() {
  try {
    const [settings, channels, scopes] = await Promise.all([
      request(SCHED),
      request(SCHED + "/channels"),
      request(SCHED + "/scopes"),
    ]);
    schedSettings = settings.settings || {};
    schedChannels = channels.channels || [];
    schedScopes = scopes.scopes || [];
  } catch (error) {
    const host = $("sched-count");
    if (host) host.textContent = error.message;
    return;
  }
  fillSettings();
  fillChannels();
  renderScopes();
  // A build outlives the page that started it: re-attach on load, so closing
  // and reopening the page keeps the progress moving (and Cancel working).
  const progress = await loadProgress();
  if ((progress || {}).state === "running") pollProgress();
  if (schedChannelId) await loadLayers();
}

function fillSettings() {
  $("sched-days").value = schedSettings.days_to_build;
  $("sched-time").value = schedSettings.build_time || "";
  $("sched-retries").value = schedSettings.max_retries;
  $("sched-pool").value = schedSettings.pool_cap;
  $("sched-enabled").checked = !!schedSettings.enabled;
  $("sched-write-playout").checked = !!schedSettings.write_playout;
  $("sched-playout-dir").value = schedSettings.playout_dir || "";
}

function fillChannels() {
  const select = $("sched-channel");
  if (!select) return;
  const current = schedChannelId || (schedChannels[0] && schedChannels[0].id) || "";
  select.textContent = "";
  schedChannels.forEach((channel) => {
    const option = document.createElement("option");
    option.value = channel.id;
    option.textContent = `${channel.number} — ${channel.name}`;
    select.appendChild(option);
  });
  schedChannelId = current;
  if (current) select.value = current;
  const clearSelect = $("sched-clear-channel");
  if (clearSelect) {
    const keep = clearSelect.value;
    clearSelect.textContent = "";
    schedChannels.forEach((channel) => {
      const option = document.createElement("option");
      option.value = channel.id;
      option.textContent = `${channel.number} — ${channel.name}`;
      clearSelect.appendChild(option);
    });
    if (keep) clearSelect.value = keep;
  }
  const count = $("sched-count");
  if (count) count.textContent = `${schedChannels.length} channel(s)`;
  if (!$("sched-date").value) {
    $("sched-date").value = new Date().toISOString().slice(0, 10);
  }
}

async function loadLayers() {
  if (!schedChannelId) return;
  try {
    const [hours, rules, items, prompt] = await Promise.all([
      request(`${SCHED}/channels/${schedChannelId}/hours`),
      request(`${SCHED}/channels/${schedChannelId}/rules`),
      request(`${SCHED}/channels/${schedChannelId}/items`),
      request(`${SCHED}/channels/${schedChannelId}/prompt`),
    ]);
    $("sched-hours").value = JSON.stringify(hours || {}, null, 2);
    $("sched-rules").value = JSON.stringify(rules || [], null, 2);
    $("sched-items").value = JSON.stringify(items || {}, null, 2);
    $("sched-prompt-layer").value = typeof prompt === "string" ? prompt : "";
    $("sched-layers-state").textContent = "";
    await loadDays();
  } catch (error) {
    $("sched-layers-result").textContent = error.message;
  }
}

function parseField(id) {
  const text = $(id).value.trim();
  return text ? JSON.parse(text) : null;
}

async function saveLayers() {
  const result = $("sched-layers-result");
  if (!schedChannelId) return;
  let hours, rules, items;
  try {
    hours = parseField("sched-hours") || {};
    rules = parseField("sched-rules") || [];
    items = parseField("sched-items") || {};
  } catch (error) {
    result.textContent = "One of the JSON fields is invalid: " + error.message;
    return;
  }
  try {
    await Promise.all([
      request(`${SCHED}/channels/${schedChannelId}/hours`, { method: "PUT", body: JSON.stringify(hours) }),
      request(`${SCHED}/channels/${schedChannelId}/rules`, { method: "PUT", body: JSON.stringify(rules) }),
      request(`${SCHED}/channels/${schedChannelId}/items`, { method: "PUT", body: JSON.stringify(items) }),
      request(`${SCHED}/channels/${schedChannelId}/prompt`, {
        method: "PUT",
        body: JSON.stringify($("sched-prompt-layer").value),
      }),
    ]);
    result.textContent = "Layers saved.";
  } catch (error) {
    result.textContent = error.message;
  }
}

async function saveSettings(event) {
  if (event && event.preventDefault) event.preventDefault();
  const result = $("sched-settings-result");
  const body = {
    days_to_build: Number($("sched-days").value) || 3,
    build_time: $("sched-time").value.trim() || "02:00",
    max_retries: Number($("sched-retries").value) || 0,
    pool_cap: Number($("sched-pool").value) || 120,
    enabled: $("sched-enabled").checked,
    write_playout: $("sched-write-playout").checked,
    playout_dir: $("sched-playout-dir").value.trim(),
  };
  try {
    const data = await request(SCHED, { method: "PUT", body: JSON.stringify(body) });
    schedSettings = data.settings || body;
    result.textContent = "Saved.";
  } catch (error) {
    result.textContent = error.message;
  }
}

async function previewPrompt() {
  const pre = $("sched-prompt");
  const result = $("sched-layers-result");
  pre.hidden = false;
  pre.textContent = "Compiling…";
  try {
    const data = await request(`${SCHED}/channels/${schedChannelId}/compile?date=${$("sched-date").value}`, {
      method: "POST",
    });
    pre.textContent = data.prompt;
    result.textContent = `${data.pool.length} candidate(s), base ${data.base_hash.slice(0, 8)}`;
  } catch (error) {
    pre.textContent = "";
    result.textContent = error.message;
  }
}

async function buildDay() {
  const result = $("sched-layers-result");
  result.textContent = "Building (the model may take a minute)…";
  try {
    const data = await request(`${SCHED}/channels/${schedChannelId}/build?date=${$("sched-date").value}`, {
      method: "POST",
    });
    const outcome = data.outcome || {};
    const day = outcome.day || {};
    result.textContent = outcome.skipped
      ? schedSettings.write_playout
        ? "Unchanged — kept the stored day and rewrote its playout and guide."
        : "Unchanged — the stored day was kept."
      : `${day.degraded ? "Degraded" : "Built"} day with ${(day.blocks || []).length} block(s)${day.provider ? ` via ${day.provider}` : ""}.`;
    await loadDays();
  } catch (error) {
    result.textContent = error.message;
  }
}

async function loadDays() {
  try {
    const data = await request(`${SCHED}/channels/${schedChannelId}/days`);
    schedDays = data.days || [];
  } catch (error) {
    schedDays = [];
  }
  renderDays();
}

function renderDays() {
  const host = $("sched-days");
  if (!host) return;
  host.textContent = "";
  if (!schedDays.length) {
    host.innerHTML = '<p class="hint">No days built yet.</p>';
    return;
  }
  schedDays.forEach((day) => {
    const card = document.createElement("div");
    card.className = "day-card";
    const head = document.createElement("div");
    head.className = "day-head";
    head.innerHTML =
      `<strong>${escapeHtml(day.date)}</strong>` +
      `<span class="day-meta">${day.blocks} block(s)${day.provider ? " · " + escapeHtml(String(day.provider)) : ""}</span>` +
      (day.degraded ? '<span class="badge-degraded">degraded</span>' : "") +
      '<span class="spacer"></span>';
    const show = document.createElement("button");
    show.type = "button";
    show.className = "ghost";
    show.textContent = "Show";
    show.addEventListener("click", () => toggleDay(card, day.date));
    const playout = document.createElement("button");
    playout.type = "button";
    playout.className = "ghost";
    playout.textContent = "Playout";
    playout.addEventListener("click", () => writePlayout(day.date));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "ghost";
    remove.textContent = "Delete";
    remove.addEventListener("click", () => deleteDay(day.date));
    head.appendChild(show);
    head.appendChild(playout);
    head.appendChild(remove);
    card.appendChild(head);
    host.appendChild(card);
  });
}

async function toggleDay(card, date) {
  if (card.dataset.open === "1") {
    card.dataset.open = "0";
    card.querySelectorAll(".block-row").forEach((row) => row.remove());
    return;
  }
  card.dataset.open = "1";
  try {
    const data = await request(`${SCHED}/channels/${schedChannelId}/days/${date}`);
    const day = data.day || {};
    (day.blocks || []).forEach((block) => {
      const row = document.createElement("div");
      row.className = "block-row";
      const start = new Date(block.start_ms).toISOString().slice(11, 16);
      const items = (block.items || [])
        .map((item) => `${escapeHtml(item.title)}${item.why ? ` <span class="why">(${escapeHtml(item.why)})</span>` : ""}`)
        .join(", ");
      row.innerHTML = `<span class="slot">${start} · ${block.span_slots}×</span><span>${items || "(empty)"}</span>`;
      card.appendChild(row);
    });
  } catch (error) {
    showToast(error.message);
  }
}

async function writePlayout(date) {
  const result = $("sched-layers-result");
  result.textContent = "Writing playout…";
  try {
    const data = await request(`${SCHED}/channels/${schedChannelId}/playout?date=${date}`, {
      method: "POST",
    });
    const report = data.report || {};
    result.textContent = `Wrote ${report.path} (${report.items} item(s), ${report.missing} slate).`;
  } catch (error) {
    result.textContent = error.message;
  }
}

async function deleteDay(date) {  if (!window.confirm(`Delete the day ${date}?`)) return;
  try {
    await request(`${SCHED}/channels/${schedChannelId}/days/${date}`, { method: "DELETE" });
    await loadDays();
  } catch (error) {
    showToast(error.message);
  }
}

// ── cancel and clear ─────────────────────────────────────────────────────────

async function cancelBuild() {
  const result = $("sched-settings-result");
  try {
    await request(`${SCHED}/build/cancel`, { method: "POST" });
    if (result) result.textContent = "Cancelling the build…";
    setTimeout(pollProgress, 500);
  } catch (error) {
    if (result) result.textContent = error.message;
  }
}

async function clearChannel() {
  const select = $("sched-clear-channel");
  const result = $("sched-maintenance-result");
  const id = select ? select.value : "";
  if (!id) return;
  const name = (select.options[select.selectedIndex] || {}).textContent || id;
  if (!window.confirm(`Clear the schedule and guide for ${name}?`)) return;
  try {
    const data = await request(`${SCHED}/channels/${id}/clear`, { method: "POST" });
    result.textContent = `Cleared ${data.cleared_days} day(s).`;
    if (schedChannelId === id) await loadDays();
  } catch (error) {
    result.textContent = error.message;
  }
}

async function clearGuide() {
  const result = $("sched-maintenance-result");
  if (!window.confirm("Clear the whole guide for every channel?")) return;
  try {
    const data = await request(`${SCHED}/guide/clear`, { method: "POST" });
    result.textContent = `Cleared the guide for ${data.cleared_channels} channel(s).`;
  } catch (error) {
    result.textContent = error.message;
  }
}

// ── rule scopes ──────────────────────────────────────────────────────────────

function renderScopes() {
  const host = $("sched-scopes");
  if (!host) return;
  host.textContent = "";
  if (!schedScopes.length) {
    host.innerHTML = '<p class="hint">No rule scopes.</p>';
    return;
  }
  schedScopes.forEach((scope) => {
    const card = document.createElement("div");
    card.className = "scope-card";
    const when = scope.scope && scope.scope.when ? Object.keys(scope.scope.when).join(",") : "";
    card.innerHTML =
      `<strong>${escapeHtml(scope.name || scope.id)}</strong>` +
      `<span class="scope-meta">layer ${scope.target} · ${escapeHtml(scope.mode || "")} · ${escapeHtml(when)} · p${scope.priority}</span>` +
      '<span class="spacer"></span>';
    const edit = document.createElement("button");
    edit.type = "button";
    edit.className = "ghost";
    edit.textContent = "Edit";
    edit.addEventListener("click", () => openScopeDialog(scope));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "ghost";
    remove.textContent = "Delete";
    remove.addEventListener("click", () => deleteScope(scope.id));
    card.appendChild(edit);
    card.appendChild(remove);
    host.appendChild(card);
  });
}

const SCOPE_TEMPLATE = {
  name: "Christmas",
  enabled: true,
  priority: 0,
  scope: { channels: [], when: { kind: "holiday", id: "christmas" } },
  target: 5,
  mode: "append",
  payload: "Merry Christmas from the whole team.",
};

function openScopeDialog(scope) {
  const dialog = $("sched-scope-dialog");
  if (!dialog) return;
  $("sched-scope-title").textContent = scope ? "Edit scope" : "Add scope";
  $("sched-scope-json").value = JSON.stringify(scope || SCOPE_TEMPLATE, null, 2);
  $("sched-scope-result").textContent = "";
  dialog.dataset.id = scope ? scope.id : "";
  dialog.showModal();
}

async function saveScope() {
  const dialog = $("sched-scope-dialog");
  const result = $("sched-scope-result");
  let scope;
  try {
    scope = JSON.parse($("sched-scope-json").value);
  } catch (error) {
    result.textContent = "Invalid JSON: " + error.message;
    return;
  }
  const id = dialog.dataset.id;
  try {
    if (id) {
      await request(`${SCHED}/scopes/${id}`, { method: "PUT", body: JSON.stringify(scope) });
    } else {
      await request(`${SCHED}/scopes`, { method: "POST", body: JSON.stringify(scope) });
    }
    dialog.close();
    const data = await request(`${SCHED}/scopes`);
    schedScopes = data.scopes || [];
    renderScopes();
  } catch (error) {
    result.textContent = error.message;
  }
}

async function deleteScope(id) {
  if (!window.confirm("Delete this scope?")) return;
  try {
    await request(`${SCHED}/scopes/${id}`, { method: "DELETE" });
    const data = await request(`${SCHED}/scopes`);
    schedScopes = data.scopes || [];
    renderScopes();
  } catch (error) {
    showToast(error.message);
  }
}

// ── horizon ──────────────────────────────────────────────────────────────────

async function buildHorizon() {
  const note = $("sched-settings-result");
  try {
    const data = await request(`${SCHED}/build`, { method: "POST" });
    note.textContent = data.status === "already running" ? "A build is already running." : "Horizon build started.";
    pollProgress();
  } catch (error) {
    note.textContent = error.message;
  }
}

async function loadProgress() {
  try {
    const data = await request(`${SCHED}/build/status`);
    const progress = data.progress || {};
    renderProgress(progress);
    return progress;
  } catch (error) {
    /* leave the label alone */
    return null;
  }
}

function renderProgress(progress) {
  const label = $("sched-progress");
  if (!label) return;
  if (progress.state === "running") {
    label.textContent = `building ${progress.channel_index}/${progress.channel_total} · day ${progress.day_index}/${progress.day_total}`;
  } else if (progress.state === "cancelled") {
    label.textContent = "cancelled";
  } else {
    label.textContent = `${progress.built || 0} built · ${progress.skipped || 0} skipped · ${progress.degraded || 0} degraded`;
  }
}

// Keep the "Generated days" heading current while a build runs. Started when a
// build is launched and re-attached on page load, so closing and reopening the
// page never loses a build that is still going. No poll limit — a horizon
// build over many channels can take a long time.
let schedPolling = false;

async function pollProgress() {
  if (schedPolling) return;
  schedPolling = true;
  try {
    for (;;) {
      const data = await request(`${SCHED}/build/status`).catch(() => null);
      if (!data) return;
      const progress = data.progress || {};
      renderProgress(progress);
      if (progress.state !== "running") {
        if (schedChannelId) await loadDays();
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }
  } finally {
    schedPolling = false;
  }
}

{
  const settingsForm = $("sched-settings-form");
  if (settingsForm) settingsForm.addEventListener("submit", saveSettings);
  const channel = $("sched-channel");
  if (channel) {
    channel.addEventListener("change", async () => {
      schedChannelId = channel.value;
      await loadLayers();
    });
  }
  const saveLayersButton = $("sched-save-layers");
  if (saveLayersButton) saveLayersButton.addEventListener("click", saveLayers);
  const preview = $("sched-preview");
  if (preview) preview.addEventListener("click", previewPrompt);
  const build = $("sched-build-day");
  if (build) build.addEventListener("click", buildDay);
  const horizon = $("sched-build-horizon");
  if (horizon) horizon.addEventListener("click", buildHorizon);
  const cancelHorizon = $("sched-cancel-horizon");
  if (cancelHorizon) cancelHorizon.addEventListener("click", cancelBuild);
  const cancelDay = $("sched-cancel-day");
  if (cancelDay) cancelDay.addEventListener("click", cancelBuild);
  const clearChannelButton = $("sched-clear-channel-btn");
  if (clearChannelButton) clearChannelButton.addEventListener("click", clearChannel);
  const clearGuideButton = $("sched-clear-guide");
  if (clearGuideButton) clearGuideButton.addEventListener("click", clearGuide);
  const addScope = $("sched-scope-add");
  if (addScope) addScope.addEventListener("click", () => openScopeDialog(null));
  const saveScopeButton = $("sched-scope-save");
  if (saveScopeButton) saveScopeButton.addEventListener("click", saveScope);
  const closeScope = $("sched-scope-close");
  if (closeScope) closeScope.addEventListener("click", () => $("sched-scope-dialog").close());
  const scopeDialog = $("sched-scope-dialog");
  if (scopeDialog) {
    scopeDialog.addEventListener("click", (event) => {
      if (event.target === scopeDialog) scopeDialog.close();
    });
  }
}

CF.define("scheduler", { onShow: loadScheduler });
