// Presets: ready-made Binarygeek119 channels to stand up quickly. The plugin
// lists presets and which are already covered; "Create missing channels" adds
// the missing ones through the base's own channel API, so presets are a
// shortcut — never the only way to add channels.

const PRESETS_ROUTE = "/api/plugins/com.channelflow.presets/presets";
const PRESETS_WEB = "/plugin/com.channelflow.presets/web";
let presetRows = [];
let presetMode = "retro";
// The selected WeatherStar (channelflow-weatherstar4000/3000) and the switch
// options, provided only while both are contributed (Weather plugin installed).
let presetWeatherStar = null;
let presetWeatherStarOptions = null;

// A preset's logo URL: absolute when its owning plugin serves it, else
// pieced onto this plugin's web root.
function presetLogoUrl(logo) {
  if (!logo) return null;
  if (logo.startsWith("/")) return logo;
  const encoded = logo.split("/").map((part) => encodeURIComponent(part)).join("/");
  return `${PRESETS_WEB}/${encoded}`;
}

function logoCell(logo) {
  const url = presetLogoUrl(logo);
  return url
    ? `<td class="preset-logo"><img src="${escapeHtml(url)}" alt="" loading="lazy" title="Watermark logo"></td>`
    : "<td class=\"preset-logo\"></td>";
}

// A preset's logo as a data URL, to upload as the created channel's watermark.
async function logoDataUrl(logo) {
  const url = presetLogoUrl(logo);
  if (!url) return null;
  try {
    const response = await fetch(url);
    if (!response.ok) return null;
    const blob = await response.blob();
    return new Promise((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || ""));
      reader.readAsDataURL(blob);
    });
  } catch (error) {
    return null;
  }
}

function renderWeatherStarSwitch() {
  const row = $("weatherstar-modes");
  if (!row) return;
  const options = presetWeatherStarOptions ? presetWeatherStarOptions.options || [] : [];
  if (!options.length || !presetWeatherStar) {
    row.hidden = true;
    row.replaceChildren();
    return;
  }
  // Exactly one WeatherStar is a preset row; the switch picks which. Off =
  // WeatherStar4000, on = WeatherStar3000.
  const on = presetWeatherStar === "channelflow-weatherstar3000";
  row.hidden = false;
  row.className = "presets-modes presets-dial-switch";
  row.replaceChildren();
  const label = document.createElement("span");
  label.className = "field-label";
  label.textContent = "WeatherStar";
  row.appendChild(label);
  const wrap = document.createElement("span");
  wrap.className = "switch-with-labels";
  const left = document.createElement("span");
  left.className = "switch-label";
  left.textContent = "4000";
  const right = document.createElement("span");
  right.className = "switch-label";
  right.textContent = "3000";
  const sw = document.createElement("label");
  sw.className = "switch";
  sw.title = "WeatherStar4000 ⇄ WeatherStar3000";
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = on;
  input.addEventListener("change", () => {
    loadPresets(
      presetMode,
      input.checked ? "channelflow-weatherstar3000" : "channelflow-weatherstar4000"
    );
  });
  const track = document.createElement("span");
  track.className = "track";
  const thumb = document.createElement("span");
  thumb.className = "thumb";
  sw.appendChild(input);
  sw.appendChild(track);
  sw.appendChild(thumb);
  wrap.appendChild(left);
  wrap.appendChild(sw);
  wrap.appendChild(right);
  row.appendChild(wrap);
}

function renderPresets() {
  renderWeatherStarSwitch();
  const list = $("presets-list");
  if (!list) return;
  const count = $("presets-count");
  if (count) count.textContent = `${presetRows.length} channel preset(s)`;
  list.innerHTML =
    `<table class="data-table">` +
    `<thead><tr><th>Ch</th><th>Logo</th><th>Name</th><th>Type</th><th>Description</th><th></th></tr></thead>` +
    `<tbody>` +
    presetRows
      .map(
        (preset) =>
          `<tr>` +
          `<td>${escapeHtml(String(preset.number))}</td>` +
          logoCell(preset.logo) +
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
  const dial = $("presets-dial-switch");
  if (dial) dial.checked = presetMode === "modern";
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
      const payload = {
        number: preset.number,
        name: preset.name,
        description: preset.description,
        enabled: true,
      };
      // The v1 logo becomes the created channel's watermark.
      const dataUrl = await logoDataUrl(preset.logo);
      if (dataUrl) {
        payload.watermark = {
          image_data: dataUrl,
          position: "bottom-right",
          offset_x: 12,
          offset_y: 12,
          scale: 100,
          opacity: 100,
          fade_before_break: false,
          fade_back_after_break: false,
          fade_back_delay_seconds: 5,
          fade_duration_seconds: 1.5,
        };
      }
      const createdChannel = await request("/api/channels", {
        method: "POST",
        body: JSON.stringify(payload),
      });
      created += 1;
      // Give the Scheduler something to build from. The Scheduler may not be
      // installed, so a missing route is not an error — the channel still
      // exists, just with empty layers.
      if (preset.layers && createdChannel && createdChannel.id) {
        try {
          await request(
            `/api/plugins/com.channelflow.scheduler/channels/${createdChannel.id}/layers`,
            { method: "POST", body: JSON.stringify(preset.layers) }
          );
        } catch (error) {
          /* no Scheduler, or it refused the bundle */
        }
      }
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
  const dial = $("presets-dial-switch");
  if (dial) {
    dial.addEventListener("change", () => {
      loadPresets(dial.checked ? "modern" : "retro", presetWeatherStar);
    });
  }
}

CF.define("presets", { onShow: () => loadPresets(presetMode, presetWeatherStar) });