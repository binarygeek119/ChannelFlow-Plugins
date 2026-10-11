// Presets: ready-made Binarygeek119 channels to stand up quickly. The plugin
// lists presets and which are already covered; "Create missing channels" adds
// the missing ones through the base's own channel API, so presets are a
// shortcut — never the only way to add channels.

const PRESETS_ROUTE = "/api/plugins/com.channelflow.presets/presets";
let presetRows = [];
let presetMode = "retro";
// The selected WeatherStar (channelflow-weatherstar4000/3000) and the switch
// options, provided only while both are contributed (Weather plugin installed).
let presetWeatherStar = null;
let presetWeatherStarOptions = null;

function renderWeatherStarSwitch() {
  const row = $("weatherstar-modes");
  if (!row) return;
  const options = presetWeatherStarOptions ? presetWeatherStarOptions.options || [] : [];
  if (!options.length || !presetWeatherStar) {
    row.hidden = true;
    row.replaceChildren();
    return;
  }
  row.hidden = false;
  row.replaceChildren();
  const label = document.createElement("span");
  label.className = "field-label";
  label.textContent = "WeatherStar";
  row.appendChild(label);
  options.forEach((option) => {
    const item = document.createElement("label");
    item.className = "presets-mode-toggle";
    const input = document.createElement("input");
    input.type = "radio";
    input.name = "weatherstar";
    input.value = option.id;
    input.checked = option.id === presetWeatherStar;
    input.addEventListener("change", () => {
      if (input.checked) loadPresets(presetMode, option.id);
    });
    item.appendChild(input);
    item.appendChild(document.createTextNode(` ${option.name} (${option.retro}/${option.modern})`));
    row.appendChild(item);
  });
}

function renderPresets() {
  renderWeatherStarSwitch();
  const list = $("presets-list");
  if (!list) return;
  const count = $("presets-count");
  if (count) count.textContent = `${presetRows.length} channel preset(s)`;
  list.innerHTML =
    `<table class="data-table">` +
    `<thead><tr><th>Ch</th><th>Name</th><th>Type</th><th>Description</th><th></th></tr></thead>` +
    `<tbody>` +
    presetRows
      .map(
        (preset) =>
          `<tr>` +
          `<td>${escapeHtml(String(preset.number))}</td>` +
          `<td><strong>${escapeHtml(preset.name)}</strong></td>` +
          `<td>${escapeHtml(preset.category || "")}</td>` +
          `<td>${escapeHtml(preset.description || "")}</td>` +
          `<td class="status ${preset.exists ? "existing" : "new"}">${preset.exists ? "exists" : "new"}</td>` +
          `</tr>`
      )
      .join("") +
    `</tbody></table>`;
  const note = $("presets-note");
  if (note) {
    const style = presetMode === "modern" ? "modern subchannel" : "retro whole-number";
    note.textContent = presetRows.length
      ? `Showing ${style} dial numbers.`
      : "The ready-made lineup is not available.";
  }
  const apply = $("presets-apply");
  if (apply) apply.disabled = !presetRows.some((preset) => !preset.exists);
}

async function loadPresets(mode, weatherStar) {
  presetMode = mode || "retro";
  if (weatherStar) presetWeatherStar = weatherStar;
  document.querySelectorAll('input[name="presets-mode"]').forEach((radio) => {
    radio.checked = radio.value === presetMode;
  });
  try {
    let url = `${PRESETS_ROUTE}?mode=${encodeURIComponent(presetMode)}`;
    if (presetWeatherStar) url += `&weatherstar=${encodeURIComponent(presetWeatherStar)}`;
    const data = await request(url);
    presetRows = data.presets || [];
    presetWeatherStarOptions = data.weatherstar || null;
    // The switch only appears while the Weather plugin contributes both
    // WeatherStars; the server answers with the selected one.
    if (presetWeatherStarOptions && presetWeatherStarOptions.selected) {
      presetWeatherStar = presetWeatherStarOptions.selected;
    } else {
      presetWeatherStar = null;
    }
    const note = $("presets-note");
    if (note && data.note) note.textContent = data.note;
  } catch (error) {
    presetRows = [];
    presetWeatherStarOptions = null;
    const note = $("presets-note");
    if (note) note.textContent = error.message;
  }
  renderPresets();
}

async function applyPresets() {
  const result = $("presets-result");
  const missing = presetRows.filter((preset) => !preset.exists);
  if (!missing.length) {
    if (result) result.textContent = "All presets already exist — nothing to create.";
    return;
  }
  const button = $("presets-apply");
  if (button) button.disabled = true;
  if (result) result.textContent = `Creating ${missing.length} channel(s)…`;
  let created = 0;
  let skipped = 0;
  for (const preset of missing) {
    try {
      await request("/api/channels", {
        method: "POST",
        body: JSON.stringify({
          number: preset.number,
          name: preset.name,
          description: preset.description,
          enabled: true,
        }),
      });
      created += 1;
    } catch (error) {
      skipped += 1;
    }
  }
  await loadPresets(presetMode, presetWeatherStar);
  if (result) {
    result.textContent = `Created ${created} channel(s), skipped ${skipped} (taken or invalid).`;
  }
}

{
  const apply = $("presets-apply");
  if (apply) apply.addEventListener("click", applyPresets);
  document.querySelectorAll('input[name="presets-mode"]').forEach((radio) => {
    radio.addEventListener("change", () => {
      if (radio.checked) loadPresets(radio.value, presetWeatherStar);
    });
  });
}

CF.define("presets", { onShow: () => loadPresets(presetMode, presetWeatherStar) });