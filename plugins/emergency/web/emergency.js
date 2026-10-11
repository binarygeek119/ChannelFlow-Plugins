const EMERGENCY_API = "/api/plugins/com.channelflow.emergency";
const EMERGENCY_WEB = "/plugin/com.channelflow.emergency/web";

// The EBS media the channel uses, carried over from v1.0.0 (offline screens
// live with the Off Air plugin).
const EBS_MEDIA = [
  { file: "media/EBS/ebs_fullscreen.png", label: "Fullscreen alert", wide: true },
  { file: "media/EBS/ebs_strip.png", label: "Alert strip" },
];

function renderEbsMedia() {
  const box = $("ebs-media");
  if (!box) return;
  box.innerHTML = EBS_MEDIA.map(
    (item) =>
      `<figure class="ebs-media-item${item.wide ? " wide" : ""}">` +
      `<img src="${EMERGENCY_WEB}/${item.file.split("/").map(encodeURIComponent).join("/")}" alt="${escapeHtml(item.label)}" loading="lazy">` +
      `<figcaption>${escapeHtml(item.label)}</figcaption>` +
      `</figure>`
  ).join("");
}

function emergencyDisplayLabel(value) {
  const labels = {
    off: "Off",
    cutin: "Switch to the alerts screen every so often",
    ticker: "Scrolling alert text at the bottom",
  };
  return labels[value] || String(value);
}

function syncEmergencyFields() {
  const cutin = $("alert-display") && $("alert-display").value === "cutin";
  const interval = $("alert-cutin-fields");
  const duration = $("alert-cutin-duration-field");
  if (interval) interval.hidden = !cutin;
  if (duration) duration.hidden = !cutin;
}

function fillEmergencyDisplay(selected) {
  const select = $("alert-display");
  if (!select) return;
  select.textContent = "";
  ["off", "cutin", "ticker"].forEach((value) => {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = emergencyDisplayLabel(value);
    select.appendChild(option);
  });
  if (selected) select.value = selected;
}

function collectEmergencySettings() {
  return {
    alert_display: $("alert-display").value,
    cutin_interval_minutes: Number($("alert-cutin-interval").value) || 15,
    cutin_duration_seconds: Number($("alert-cutin-duration").value) || 20,
    location: ($("alert-location").value || "").trim(),
    source: $("alert-source").value,
  };
}

async function loadEmergency() {
  renderEbsMedia();
  try {
    const data = await request(EMERGENCY_API);
    const settings = data.settings || {};
    fillEmergencyDisplay(settings.alert_display);
    const interval = $("alert-cutin-interval");
    if (interval) interval.value = settings.cutin_interval_minutes;
    const duration = $("alert-cutin-duration");
    if (duration) duration.value = settings.cutin_duration_seconds;
    const location = $("alert-location");
    if (location) location.value = settings.location || "";
    const source = $("alert-source");
    if (source) source.value = settings.source || "auto";
    syncEmergencyFields();
    $("alert-result").textContent = "";
  } catch (error) {
    $("alert-result").textContent = error.message;
  }
  renderAlertStatus();
}

// What the overlay would do right now: mode, schedule, and the active alerts.
async function renderAlertStatus() {
  const status = $("alert-status");
  if (!status) return;
  const list = $("alert-list");
  if (!list) return;
  status.textContent = "Checking alerts…";
  list.textContent = "";
  let data;
  try {
    data = await request(`${EMERGENCY_API}/alerts`);
  } catch (error) {
    status.textContent = error.message;
    return;
  }
  if (!data.ok) {
    status.textContent = data.error || "No alert status.";
    return;
  }
  const mode = data.mode || "off";
  const modeText = { off: "off", cutin: "cut-in", ticker: "ticker" }[mode] || mode;
  status.textContent =
    `${data.count || 0} active alert(s) · display is ${modeText}` +
    (mode === "cutin"
      ? ` · alert screen every ${Math.round((data.next_cut_in_seconds || 0) / 60)} minutes for ${data.cutin_duration_seconds || 0} seconds`
      : "") +
    (data.location && data.location.name ? ` · ${data.location.name}` : "");
  (data.alerts || []).forEach((alert) => {
    const card = document.createElement("div");
    card.className = "alert-card";
    const line = [
      alert.event,
      alert.severity ? `(${alert.severity})` : "",
      alert.headline,
    ].filter(Boolean).join(" — ");
    card.innerHTML =
      `<div class="alert-card-title">${escapeHtml(line)}</div>` +
      (alert.description
        ? `<div class="alert-card-body">${escapeHtml(alert.description)}</div>`
        : "") +
      (alert.ends_at
        ? `<div class="alert-card-note">Ends ${escapeHtml(alert.ends_at)}</div>`
        : "");
    list.appendChild(card);
  });
}

async function saveEmergency(event) {
  if (event && event.preventDefault) event.preventDefault();
  const save = $("alert-save");
  if (save) save.disabled = true;
  try {
    await request(EMERGENCY_API, { method: "PUT", body: JSON.stringify(collectEmergencySettings()) });
    if ($("alert-result")) $("alert-result").textContent = "Saved.";
    syncEmergencyFields();
    renderAlertStatus();
  } catch (error) {
    if ($("alert-result")) $("alert-result").textContent = error.message;
  } finally {
    if (save) save.disabled = false;
  }
}

async function testEmergency() {
  const test = $("alert-test");
  if (test) test.disabled = true;
  try {
    // Save first so the test describes what the form holds.
    await request(EMERGENCY_API, { method: "PUT", body: JSON.stringify(collectEmergencySettings()) });
    const data = await request(EMERGENCY_API + "/test");
    const result = data.result || {};
    $("alert-result").textContent = result.detail || (result.ok ? "OK" : "Off");
  } catch (error) {
    $("alert-result").textContent = error.message;
  } finally {
    if (test) test.disabled = false;
  }
}

{
  const form = $("alert-form");
  if (form) form.addEventListener("submit", saveEmergency);
  const test = $("alert-test");
  if (test) test.addEventListener("click", testEmergency);
  const display = $("alert-display");
  if (display) display.addEventListener("change", syncEmergencyFields);
  const refresh = $("alert-refresh");
  if (refresh) refresh.addEventListener("click", renderAlertStatus);
}


CF.define("emergency", { onShow: loadEmergency });
