// Presets: ready-made Binarygeek119 channels to stand up quickly. The plugin
// lists presets and which are already covered; "Create missing channels" adds
// the missing ones through the base's own channel API, so presets are a
// shortcut — never the only way to add channels.

const PRESETS_ROUTE = "/api/plugins/com.channelflow.presets/presets";
let presetRows = [];

function renderPresets() {
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
  if (note) note.textContent = presetRows.length ? "" : "The ready-made lineup is not available.";
  const apply = $("presets-apply");
  if (apply) apply.disabled = !presetRows.some((preset) => !preset.exists);
}

async function loadPresets() {
  try {
    const data = await request(PRESETS_ROUTE);
    presetRows = data.presets || [];
    const note = $("presets-note");
    if (note && data.note) note.textContent = data.note;
  } catch (error) {
    presetRows = [];
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
  await loadPresets();
  if (result) {
    result.textContent = `Created ${created} channel(s), skipped ${skipped} (taken or invalid).`;
  }
}

{
  const apply = $("presets-apply");
  if (apply) apply.addEventListener("click", applyPresets);
}

CF.define("presets", { onShow: loadPresets });