const OFFAIR_API = "/api/plugins/com.channelflow.offair";

function offAirLabel(value) {
  const labels = {
    slate_image: "Slate image",
    color_bars: "Color bars",
    static: "TV static",
    usa: "USA slate",
    international: "World slate",
    background_music: "Background music",
    white_noise: "White noise",
    silence: "Silence",
    beep_tone: "Beep tone",
    all_music_libraries: "All music libraries",
    named_library: "One music library",
    local_packs: "Local music packs",
  };
  return labels[value] || String(value).replace(/_/g, " ");
}

function fillSelect(id, values, selected) {
  const select = $(id);
  if (!select || !values) return;
  select.textContent = "";
  values.forEach((value) => {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = offAirLabel(value);
    select.appendChild(option);
  });
  if (selected) select.value = selected;
}

let offAirCustomSlates = {};

function slatePreviewUrl(variant, aspect) {
  return `${OFFAIR_API}/slates/${encodeURIComponent(variant)}/image?aspect=${encodeURIComponent(String(aspect))}`;
}

// The custom-slate editor: previews the effective slate (custom or stock) and
// uploads/removes a custom image per region, like v1.0.0's EBS slates.
function renderOffAirSlates() {
  const preview = $("ebs-slate-preview");
  if (!preview) return;
  const variant = $("ebs-slate-variant2") ? $("ebs-slate-variant2").value : "usa";
  const aspect = $("ebs-slate-aspect") ? $("ebs-slate-aspect").value : "0";
  preview.src = slatePreviewUrl(variant, aspect);
  preview.style.display = "";
  const remove = $("ebs-slate-remove");
  if (remove) remove.hidden = !offAirCustomSlates[variant];
  const note = $("ebs-slate-note");
  if (note) {
    note.textContent = offAirCustomSlates[variant]
      ? `Custom slate active (${offAirCustomSlates[variant]}).`
      : "Using the bundled stock slate for this region.";
  }
}

async function uploadOffAirSlate() {
  const file = $("ebs-slate-file") && $("ebs-slate-file").files && $("ebs-slate-file").files[0];
  if (!file) {
    setSlateNote("Choose an image first.", true);
    return;
  }
  if (!/^image\/(png|jpeg)$/.test(file.type)) {
    setSlateNote("The slate must be a PNG or JPEG image.", true);
    return;
  }
  const variant = $("ebs-slate-variant2").value;
  setSlateNote("Uploading…");
  try {
    const response = await fetch(`${OFFAIR_API}/slates/${encodeURIComponent(variant)}`, {
      method: "POST",
      body: file,
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `Upload failed (${response.status})`);
    offAirCustomSlates = data.custom_slates || offAirCustomSlates;
    setSlateNote("Custom slate saved.", false);
    renderOffAirSlates();
  } catch (error) {
    setSlateNote(error.message, true);
  }
}

async function removeOffAirSlate() {
  const variant = $("ebs-slate-variant2").value;
  try {
    const data = await request(`${OFFAIR_API}/slates/${encodeURIComponent(variant)}`, {
      method: "DELETE",
    });
    offAirCustomSlates = data.custom_slates || offAirCustomSlates;
    setSlateNote("Custom slate removed — the stock slate plays again.", false);
    renderOffAirSlates();
  } catch (error) {
    setSlateNote(error.message, true);
  }
}

function setSlateNote(message, bad) {
  const note = $("ebs-slate-note");
  if (note) {
    note.textContent = message;
    note.className = bad ? "field-note bad" : "field-note";
  }
}

function syncOffAirFields() {
  const slate = $("ebs-slate-field");
  if (slate) slate.hidden = $("ebs-display-mode").value !== "slate_image";
  const music = $("ebs-audio-mode").value === "background_music";
  const source = $("ebs-music-source-field");
  if (source) source.hidden = !music;
  const library = $("ebs-music-library-field");
  if (library) library.hidden = !music || $("ebs-music-source").value !== "named_library";
}

async function loadOffAir() {
  try {
    const data = await request(OFFAIR_API);
    const options = data.options || {};
    const settings = data.settings || {};
    fillSelect("ebs-display-mode", options.display_modes, settings.display_mode);
    fillSelect("ebs-slate-variant", options.slate_variants, settings.slate_variant);
    fillSelect("ebs-audio-mode", options.audio_modes, settings.audio_mode);
    fillSelect("ebs-music-source", options.music_sources, settings.music_source);
    $("ebs-music-library").value = settings.music_library_name || "";
    offAirCustomSlates = data.custom_slates || {};
    syncOffAirFields();
    renderOffAirSlates();
    $("ebs-result").textContent = "";
  } catch (error) {
    $("ebs-result").textContent = error.message;
  }
}

async function saveOffAir(event) {
  event.preventDefault();
  const body = {
    display_mode: $("ebs-display-mode").value,
    slate_variant: $("ebs-slate-variant").value,
    audio_mode: $("ebs-audio-mode").value,
    music_source: $("ebs-music-source").value,
    music_library_name: $("ebs-music-library").value.trim(),
    music_library_id: "",
  };
  const save = $("ebs-save");
  save.disabled = true;
  try {
    await request(OFFAIR_API, { method: "PUT", body: JSON.stringify(body) });
    $("ebs-result").textContent = "Saved.";
    syncOffAirFields();
  } catch (error) {
    $("ebs-result").textContent = error.message;
  } finally {
    save.disabled = false;
  }
}

// ── Music packs ────────────────────────────────────────────────────────────

async function loadMusicPacks() {
  const list = $("music-packs-list");
  if (!list) return;
  const note = $("music-packs-note");
  try {
    const data = await request(OFFAIR_API + "/music-packs");
    const active = data.active || "";
    list.replaceChildren();
    (data.packs || []).forEach((pack) => {
      const card = document.createElement("div");
      card.className = "music-pack" + (pack.active ? " active" : "") +
        (pack.installed ? "" : " idle");
      const name = document.createElement("strong");
      name.textContent = pack.name + (pack.active ? " · active" : "");
      const meta = document.createElement("div");
      meta.className = "music-pack-meta";
      meta.textContent = `${pack.installed ? `${pack.track_count} track${pack.track_count === 1 ? "" : "s"}` : "not downloaded"} — ${pack.plays_when}`;
      const actions = document.createElement("div");
      actions.className = "actions";
      if (pack.installed) {
        const del = document.createElement("button");
        del.type = "button";
        del.className = "ghost danger";
        del.textContent = "Delete";
        del.addEventListener("click", () => deleteMusicPack(pack.id));
        actions.appendChild(del);
      } else {
        const download = document.createElement("button");
        download.type = "button";
        download.className = "ghost";
        download.textContent = "Download";
        download.addEventListener("click", () => downloadMusicPack(pack.id));
        actions.appendChild(download);
      }
      card.append(name, meta, actions);
      list.appendChild(card);
    });
    if (data.active_track && note) {
      note.textContent = `Off-air music would play: ${data.active_track}`;
    } else if (note) {
      note.textContent = "No pack downloaded yet — download one to give off-air music a source.";
    }
  } catch (error) {
    if (note) note.textContent = error.message;
  }
}

async function downloadMusicPack(id) {
  const note = $("music-packs-note");
  if (note) note.textContent = `Downloading ${id}…`;
  try {
    const data = await request(`${OFFAIR_API}/music-packs/${encodeURIComponent(id)}/download`, { method: "POST" });
    if (note) note.textContent = data.error || `${id} ready.`;
    await loadMusicPacks();
  } catch (error) {
    if (note) note.textContent = error.message;
  }
}

async function deleteMusicPack(id) {
  if (!confirm(`Delete the ${id} music pack and its audio?`)) return;
  try {
    await request(`${OFFAIR_API}/music-packs/${encodeURIComponent(id)}`, { method: "DELETE" });
    await loadMusicPacks();
  } catch (error) {
    const note = $("music-packs-note");
    if (note) note.textContent = error.message;
  }
}

{
  const form = $("ebs-form");
  if (form) form.addEventListener("submit", saveOffAir);
  ["ebs-display-mode", "ebs-audio-mode", "ebs-music-source"].forEach((id) => {
    const select = $(id);
    if (select) select.addEventListener("change", syncOffAirFields);
  });
  const upload = $("ebs-slate-upload");
  if (upload) upload.addEventListener("click", uploadOffAirSlate);
  const remove = $("ebs-slate-remove");
  if (remove) remove.addEventListener("click", removeOffAirSlate);
  ["ebs-slate-variant2", "ebs-slate-aspect"].forEach((id) => {
    const select = $(id);
    if (select) select.addEventListener("change", renderOffAirSlates);
  });
}

CF.define("ebs", { onShow: () => { loadOffAir(); loadMusicPacks(); } });
