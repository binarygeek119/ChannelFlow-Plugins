const LIBRARY_ROUTES = {
  // `base` sources expose a list of libraries; every source has a core task
  // endpoint a per-connection Sync runs through.
  jellyfin: { base: "/api/plugins/com.channelflow.jellyfin", run: "/api/tasks/jellyfin-sync/run" },
  local: { run: "/api/tasks/local-scan/run" },
  news: { run: "/api/tasks/news-scan/run" },
};
let librarySources = [];
let libraryPage = "connections";
let libraryConnections = [];
let connectionEditingId = null;
let savingConnection = false;

async function loadLibrary() {
  try {
    librarySources = (await request("/api/mediasources")).sources || [];
  } catch (error) {
    librarySources = [];
  }
  populateConnectionKinds();
  // The kind dropdown redraws its fields on change, but nothing fires on a
  // fresh page load — draw the initially selected source's fields here so the
  // Add connection form isn't empty until you flip the kind.
  renderConnectionFields(
    $("ms-kind").value || (librarySources[0] && librarySources[0].type_id) || "",
    {}
  );
  await refreshLibraryConnections();
  renderLibraryTabs();
  applyLibraryPath();
  updateLibraryEmptyState();
}

// The inner Library page follows the URL: /webui/library is Connections,
// /webui/library/<kind> the source, /webui/library/<kind>/<id> one connection.
function applyLibraryPath() {
  const match = location.pathname.match(/^\/webui\/library\/([^/]+)(?:\/(\d+))?$/);
  if (match) {
    const kind = match[1];
    const id = match[2] ? Number(match[2]) : null;
    const wanted = id != null ? connPageId(kind, id) : kind;
    if (document.querySelector(`#library-inner-tabs .inner-tab[data-library-page="${wanted}"]`)) {
      libraryPage = wanted;
      showLibraryPage(wanted);
      return;
    }
  }
  if (location.pathname === "/webui/library") {
    libraryPage = "connections";
    showLibraryPage("connections");
  }
}

async function refreshLibraryConnections() {
  try {
    libraryConnections = (await request("/api/connections")).connections || [];
  } catch (error) {
    libraryConnections = [];
  }
}

function sourceName(kind) {
  const source = librarySources.find((entry) => entry.type_id === kind);
  return source ? source.display_name : kind;
}

function populateConnectionKinds() {
  const select = $("ms-kind");
  if (!select) return;
  select.textContent = "";
  librarySources.forEach((source) => {
    const option = document.createElement("option");
    option.value = source.type_id;
    option.textContent = source.display_name;
    select.appendChild(option);
  });
  // Disabled while no media-source plugin is installed — there is nothing to
  // pick, and the note under the form says what to do.
  select.disabled = librarySources.length === 0;
}

function updateLibraryEmptyState() {
  const note = $("ms-result");
  if (!note) return;
  if (librarySources.length === 0) {
    note.textContent =
      "No media-source plugins are installed. Install one — for example the Jellyfin Media Source — from the Plugins page, then reload.";
  } else if (note.textContent.startsWith("No media-source plugins")) {
    note.textContent = "";
  }
}

function renderLibraryTabs() {
  const tabs = $("library-inner-tabs");
  if (!tabs) return;
  tabs.textContent = "";
  // Each tab is a real URL under Library: Connections at /webui/library, the
  // source at /webui/library/<kind>, and one tab per saved connection at
  // /webui/library/<kind>/<id> - so a connection is a tab you can browse to
  // and bookmark.
  const add = (page, label, path) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "inner-tab";
    button.dataset.libraryPage = page;
    button.textContent = label;
    button.addEventListener("click", () => {
      history.pushState(null, "", path || "/webui/library");
      showLibraryPage(page);
    });
    tabs.appendChild(button);
  };
  add("connections", "Connections", "/webui/library");
  // A per-source tab only exists once one of its connections is saved — no
  // empty tabs for installed-but-unconfigured sources. Each connection of a
  // source then gets its own tab under it.
  librarySources.forEach((source) => {
    const connections = libraryConnections.filter(
      (connection) => connection.kind === source.type_id
    );
    if (!connections.length) return;
    // One tab per connection (deduped by id, unnamed connections labeled so
    // they don't read as "the source tab twice"). A connection created with
    // the source's display name (e.g. a "Past Tense News" connection) already
    // produces that tab, so the source tab is only added when it actually
    // groups several connections and none of their tabs already carry its name.
    const seen = new Set();
    const named = [];
    connections.forEach((connection) => {
      if (!connection.id || seen.has(connection.id)) return;
      seen.add(connection.id);
      const label =
        (connection.config && connection.config.name) ||
        source.display_name + " — new";
      named.push({ connection, label });
    });
    if (
      named.length > 1 &&
      !named.some((row) => row.label === source.display_name)
    ) {
      add(source.type_id, source.display_name, `/webui/library/${source.type_id}`);
    }
    named.forEach(({ connection, label }) => {
      add(connPageId(source.type_id, connection.id), label, `/webui/library/${source.type_id}/${connection.id}`);
    });
  });
  tabs.querySelectorAll(".inner-tab").forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.libraryPage === libraryPage);
  });
}

function connPageId(kind, id) {
  return `conn-${kind}-${id}`;
}

function libraryHint(kind) {
  const route = LIBRARY_ROUTES[kind];
  if (route && route.base) {
    return "Sync the libraries each connection exposes. They are grouped by type; toggle a row to include or exclude that library from syncs.";
  }
  return "Scan each connection's media folders with Sync now — files and their .nfo metadata become one card per item on the Media page.";
}

function kindPage(kind, heading) {
  const id = `library-page-${kind}`;
  let page = $(id);
  if (!page) {
    page = document.createElement("div");
    page.id = id;
    page.className = "library-page";
    page.hidden = true;
    page.innerHTML =
      `<div class="panel"><div class="panel-head"><h2></h2><span class="count"></span></div>` +
      `<p class="hint">${libraryHint(kind)}</p>` +
      `<div class="library-lists"></div></div>`;
    $("tab-library").appendChild(page);
  }
  page.querySelector("h2").textContent = heading;
  return page;
}

function showLibraryPage(page) {
  libraryPage = page;
  document.querySelectorAll("#library-inner-tabs .inner-tab").forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.libraryPage === page);
  });
  document.querySelectorAll("#tab-library .library-page").forEach((element) => {
    element.hidden = element.id !== `library-page-${page}`;
  });
  if (page === "connections") renderConnections();
  else if (page.startsWith("conn-")) renderKindLibraryConnection(page);
  else renderKindLibraries(page);
}

// A per-connection page shows just that connection's libraries.
function renderKindLibraryConnection(page) {
  const rest = page.slice("conn-".length);
  const dash = rest.lastIndexOf("-");
  const kind = rest.slice(0, dash);
  const id = Number(rest.slice(dash + 1));
  const connection = libraryConnections.find(
    (row) => row.id === id && row.kind === kind
  );
  const heading = connection && connection.config && connection.config.name
    ? connection.config.name
    : kind;
  const pid = `library-page-${page}`;
  let pageEl = $(pid);
  if (!pageEl) {
    pageEl = document.createElement("div");
    pageEl.id = pid;
    pageEl.className = "library-page";
    pageEl.hidden = true;
    pageEl.innerHTML =
      `<div class="panel"><div class="panel-head"><h2></h2><span class="count"></span></div>` +
      `<p class="hint">${libraryHint(kind)}</p>` +
      `<div class="library-lists"></div></div>`;
    $("tab-library").appendChild(pageEl);
  }
  pageEl.querySelector("h2").textContent = heading;
  const list = pageEl.querySelector(".library-lists");
  list.textContent = "";
  if (!connection) {
    list.appendChild(libraryCard('<p class="hint">Connection removed.</p>'));
    return;
  }
  renderConnectionLibraries(list, connection);
}

function renderCurrentLibraryPage() {
  showLibraryPage(libraryPage);
}

function setCardStatus(card, result) {
  const status = card.querySelector(".ms-status");
  if (!status) return;
  const ok = result && result.ok;
  status.className = "ms-status" + (ok ? " ok" : " bad");
  status.textContent = result.detail || (ok ? "OK" : "Failed");
}

function restoreButton(button) {
  button.textContent = button.dataset.label || button.textContent;
}

function connectionActions(card, connection) {
  const actions = document.createElement("div");
  actions.className = "ms-actions";

  const run = async (button, busyLabel, work, done) => {
    button.disabled = true;
    button.textContent = busyLabel;
    try {
      done(await work());
    } catch (error) {
      setCardStatus(card, { ok: false, detail: error.message });
      restoreButton(button);
    } finally {
      button.disabled = false;
    }
  };

  if (LIBRARY_ROUTES[connection.kind]) {
    const sync = document.createElement("button");
    sync.type = "button";
    sync.className = "primary";
    sync.dataset.label = "Sync now";
    sync.textContent = "Sync now";
    sync.onclick = () =>
      run(sync, "Syncing…", () => syncConnection(connection), (report) => {
        setCardStatus(card, {
          ok: report.errors === 0,
          detail: `Synced: ${report.added} added · ${report.updated} updated · ${report.errors} errors`,
        });
        restoreButton(sync);
      });
    actions.appendChild(sync);
  }

  const test = document.createElement("button");
  test.type = "button";
  test.dataset.label = "Test";
  test.textContent = "Test";
  test.onclick = () =>
    run(test, "Testing…",
      async () => (await request(`/api/connections/${connection.id}/test`, { method: "POST" })).result,
      (result) => { setCardStatus(card, result); restoreButton(test); });
  const edit = document.createElement("button");
  edit.type = "button";
  edit.dataset.label = "Edit";
  edit.textContent = "Edit";
  edit.onclick = () => openConnectionForm(connection);
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "danger";
  remove.dataset.label = "Delete";
  remove.textContent = "Delete";
  remove.onclick = () =>
    run(remove, "Deleting…", async () => {
      if (!confirm(`Delete this connection? Its synced rows cascade and orphan posters are swept.`)) return "cancelled";
      await request(`/api/connections/${connection.id}`, { method: "DELETE" });
      await refreshLibraryConnections();
      renderLibraryTabs();
      renderCurrentLibraryPage();
      return "deleted";
    }, () => { if (card.parentNode) card.remove(); });
  actions.append(test, edit, remove);
  return actions;
}

function connectionCard(connection) {
  const card = document.createElement("div");
  card.className = "ms-card";
  const config = connection.config || {};
  const head = document.createElement("div");
  head.className = "ms-card-head";
  const title = document.createElement("h4");
  title.textContent = config.name || "(unnamed connection)";
  const kind = document.createElement("span");
  kind.className = "ms-kind";
  kind.textContent = sourceName(connection.kind) || connection.kind;
  head.append(title, kind);
  const meta = document.createElement("div");
  meta.className = "ms-meta";
  const folders = Array.isArray(config.folders) ? config.folders : [];
  const where = folders.length ? folders.join(" · ") : config.url || "";
  const whereHtml = /^https?:\/\//i.test(where)
    ? `<a href="${escapeHtml(where)}" target="_blank" rel="noopener" class="ms-url">${escapeHtml(where)}</a>`
    : escapeHtml(where || "no location");
  meta.innerHTML =
    whereHtml +
    (config.enabled === false ? `<span class="ms-status">disabled</span>` : "") +
    `<span class="ms-status">Not tested</span>`;
  card.append(head, meta, connectionActions(card, connection));
  return card;
}

function renderConnections() {
  const list = $("ms-connection-list");
  if (!list) return;
  const count = $("ms-count");
  if (count) count.textContent = `${libraryConnections.length} connection(s)`;
  list.textContent = "";
  if (!libraryConnections.length) {
    list.innerHTML = '<div class="card section-card"><p class="hint">No media server connections yet. Add one below.</p></div>';
    return;
  }
  libraryConnections.forEach((connection) => list.appendChild(connectionCard(connection)));
}

// The grouped labels and order for Jellyfin's collection types. 3D movies
// and regular movies both arrive as collection_type "movies", so they share
// the Movies box.
const LIBRARY_TYPE_LABELS = {
  movies: "Movies",
  tvshows: "TV shows",
  music: "Music",
  musicvideos: "Music videos",
};
const LIBRARY_TYPE_ORDER = ["movies", "tvshows", "music", "musicvideos"];

function libraryTypeLabel(type) {
  return LIBRARY_TYPE_LABELS[type] || type || "Other";
}

// libraryCard() is the shell's shared helper (used by the Media page too).

async function renderKindLibraries(kind) {
  const page = kindPage(kind, sourceName(kind) || kind);
  const list = page.querySelector(".library-lists");
  const count = page.querySelector(".count");
  const rows = libraryConnections.filter((connection) => connection.kind === kind);
  list.textContent = "";
  if (!rows.length) {
    if (count) count.textContent = "";
    list.appendChild(
      libraryCard(
        '<p class="hint">No connections yet — add one on the Connections tab.</p>'
      )
    );
    return;
  }
  if (count) count.textContent = `${rows.length} connection(s)`;
  await Promise.all(rows.map((connection) => renderConnectionLibraries(list, connection)));
}

async function renderConnectionLibraries(list, connection) {
  const route = LIBRARY_ROUTES[connection.kind];
  const config = connection.config || {};
  const apiKey = config.api_key || "";
  let libraries = [];
  if (route && route.base) {
    let data = null;
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        data = await request(route.base + "/libraries", {
          method: "POST",
          body: JSON.stringify({ connection: config, api_key: apiKey }),
        });
        break;
      } catch (error) {
        if (attempt === 1) {
          list.appendChild(
            libraryCard(
              `<p class="hint bad">${escapeHtml(config.name || "connection")}: ${escapeHtml(error.message)}</p>`
            )
          );
          return;
        }
      }
    }
    libraries = (data && data.libraries) || [];
  }
  const enabled = new Set(
    Array.isArray(config.enabled_libraries)
      ? config.enabled_libraries
      : libraries.map((library) => library.remote_id)
  );
  const grouped = new Map();
  for (const library of libraries) {
    const type = library.collection_type || "other";
    if (!grouped.has(type)) grouped.set(type, []);
    grouped.get(type).push(library);
  }
  const types = [...grouped.keys()].sort((a, b) => {
    const ia = LIBRARY_TYPE_ORDER.indexOf(a);
    const ib = LIBRARY_TYPE_ORDER.indexOf(b);
    return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib) || String(a).localeCompare(String(b));
  });

  const box = document.createElement("div");
  box.className = "library-box";
  const name = escapeHtml(config.name || "Connection");
  const sync = LIBRARY_ROUTES[connection.kind]
    ? `<button type="button" class="primary" data-lib-sync="${connection.id}">Sync now</button>`
    : "";
  const browse = config.url
    ? `<a href="${escapeHtml(config.url)}" target="_blank" rel="noopener" class="link">Browse server</a>`
    : "";
  box.innerHTML =
    `<div class="library-box-head"><h3>${name}</h3><div class="libbox-actions">${browse}${sync}</div></div>`;
  if (!libraries.length) {
    box.innerHTML += route && route.base
      ? '<p class="hint">This server exposes no libraries (or the key cannot list them).</p>'
      : '<p class="hint">File source — Sync now walks this connection\'s folders into the media database.</p>';
  } else {
    types.forEach((type) => {
      const label = libraryTypeLabel(type);
      box.innerHTML += `<h4 class="library-type">${escapeHtml(label)}</h4>`;
      grouped.get(type).forEach((library) => {
        const checked = enabled.has(library.remote_id);
        box.innerHTML +=
          `<div class="lib-row">
             <span class="lib-name">${escapeHtml(library.name)}</span>
             <label class="switch" title="${checked ? "Included in syncs" : "Excluded from syncs"}">
               <input type="checkbox" class="lib-toggle" data-conn="${connection.id}" data-lib="${escapeHtml(library.remote_id)}" ${checked ? "checked" : ""}>
               <span class="track"></span><span class="thumb"></span>
             </label>
           </div>`;
      });
    });
  }
  list.appendChild(box);
}

// A library toggle persisted to the connection, then re-renders so the tabs
// and any syncs reflect the new selection.
async function toggleLibrary(connectionId, remoteId, enabledNow, list) {
  const connection = libraryConnections.find((row) => row.id === connectionId);
  if (!connection) return;
  const config = { ...(connection.config || {}) };
  // Build the enabled set from what is on screen right now, so the very first
  // toggle (before any selection was stored) starts from "all of them" rather
  // than an empty list.
  const toggles = [...document.querySelectorAll(`.lib-toggle[data-conn="${connectionId}"]`)];
  config.enabled_libraries = toggles
    .filter((toggle) => toggle.checked)
    .map((toggle) => toggle.dataset.lib);
  try {
    await request(`/api/connections/${connectionId}`, {
      method: "PUT",
      body: JSON.stringify({ config }),
    });
    await refreshLibraryConnections();
    renderLibraryTabs();
    renderKindLibraries(connection.kind);
    // A library that just got turned on starts the scan for this connection,
    // so the toggled-on libraries sync right away.
    if (enabledNow) {
      taskPopup.run(
        `Jellyfin · ${escapeHtml(config.name || "connection")} scan`,
        () =>
          request("/api/tasks/jellyfin-sync/run", {
            method: "POST",
            body: JSON.stringify({ connection_id: connectionId }),
          }),
        (data) => {
          const run = data.run || {};
          return `Scan finished: ${run.added || 0} added · ${run.updated || 0} updated · ${run.errors || 0} errors`;
        }
      );
    }
  } catch (error) {
    setCardStatus(list, { ok: false, detail: error.message });
  }
}

// The toggles and the Sync button live on the library pages.
document.getElementById("tab-library").addEventListener("change", (event) => {
  const toggle = event.target.closest(".lib-toggle");
  if (toggle) toggleLibrary(Number(toggle.dataset.conn), toggle.dataset.lib, toggle.checked, document.getElementById("tab-library"));
});
document.getElementById("tab-library").addEventListener("click", async (event) => {
  const button = event.target.closest("[data-lib-sync]");
  if (!button) return;
  const connection = libraryConnections.find((row) => row.id === Number(button.dataset.libSync));
  if (!connection) return;
  taskPopup.run(
    `${sourceName(connection.kind) || "Library"} · ${escapeHtml(connection.config.name || "connection")} sync`,
    () => syncConnection(connection),
    (report) => `Synced: ${report.added} added · ${report.updated} updated · ${report.errors} errors`
  );
});

async function syncConnection(connection) {
  const config = connection.config || {};
  const route = LIBRARY_ROUTES[connection.kind];
  if (!route) throw new Error(`no sync built for ${connection.kind}`);

  // Sync through the core's task driver: for a media server it filters to the
  // toggled-on libraries, for a file source to the connection's folders, and
  // every run reports into the base's media catalog the same way the scheduled
  // scans do — so a manual Sync also feeds the Media page.
  const done = request(route.run || "/api/tasks/jellyfin-sync/run", {
    method: "POST",
    body: JSON.stringify({ connection_id: connection.id }),
  });

  taskPopup.show(`Syncing ${config.name || connection.kind}`);
  // The Jellyfin source exposes a process-wide progress slot; file-source
  // scans are fast enough to just wait on the request.
  if (route.base) await pollSyncProgress(done);

  let report;
  try {
    report = await done;
  } catch (error) {
    taskPopup.finish(error.message || "Sync failed.", false);
    throw error;
  }
  report = (report && report.run) || report || {};
  taskPopup.finish(
    `Synced: ${report.added} added · ${report.updated} updated · ${report.errors} errors`,
    report.errors === 0
  );
  return report;
}

// Poll the plugin's /progress while the sync runs and push each snapshot into
// the popup: "Movies · 5 of 19,328", then "TV · 1,024 of 8,412", and so on.
// Stops the moment the sync request settles (success or failure).
async function pollSyncProgress(done) {
  const progressUrl = LIBRARY_ROUTES.jellyfin.base + "/progress";
  let settled = false;
  done.finally(() => { settled = true; });
  while (!settled) {
    try {
      const data = await request(progressUrl);
      const snapshot = (data && data.progress) || {};
      taskPopup.progress(
        `${snapshot.label || "Library"} · ${(Number(snapshot.current) || 0).toLocaleString()}` +
          ` of ${(Number(snapshot.total) || 0).toLocaleString()}`
      );
    } catch (error) { /* a transient poll failure must not kill the sync */ }
    await new Promise((resolve) => setTimeout(resolve, 800));
  }
}

/* connection form (part of the Library page) */

function linesToRemaps(text) {
  const remaps = {};
  String(text || "").split(/\r?\n/).forEach((line) => {
    const match = line.match(/^\s*(\S+)\s*->\s*(\S+)\s*$/);
    if (match) remaps[match[1]] = match[2];
  });
  return remaps;
}

function remapsToLines(remaps) {
  if (!remaps || typeof remaps !== "object") return "";
  return Object.entries(remaps).map(([from, to]) => `${from} -> ${to}`).join("\n");
}

function fieldElementId(key) {
  return "ms-fld-" + String(key).replace(/[^a-z0-9_-]/gi, "_");
}

// Build one input for a source field: text/secret input, remaps textarea,
// repeatable folder rows with an "add another" button, or a kind dropdown.
function renderSourceField(field, config) {
  const wrap = document.createElement("div");
  wrap.className = "field";
  const label = document.createElement("label");
  label.className = "field-label";
  label.textContent = field.label || field.key;
  wrap.appendChild(label);
  const value = config ? config[field.key] : undefined;
  if (field.kind === "secret") {
    const input = document.createElement("input");
    input.type = "password";
    input.id = fieldElementId(field.key);
    input.value = value || "";
    input.required = !!field.required;
    wrap.appendChild(input);
  } else if (field.kind === "remaps") {
    const textarea = document.createElement("textarea");
    textarea.id = fieldElementId(field.key);
    textarea.rows = 3;
    textarea.placeholder = "/media -> /mnt/library";
    textarea.value = remapsToLines(value);
    wrap.appendChild(textarea);
  } else if (field.kind === "pair_list") {
    // Rows of (path input, kind dropdown) so each folder has its own kind.
    const container = document.createElement("div");
    container.id = fieldElementId(field.key);
    container.className = "repeat-list";
    const paths = Array.isArray(value) ? value.slice() : value ? [String(value)] : [""];
    const kinds = Array.isArray(config && config[field.second_key])
      ? config[field.second_key]
      : [];
    const addRow = (path, kind) => {
      const row = document.createElement("div");
      row.className = "repeat-row-pair";
      const input = document.createElement("input");
      input.type = "text";
      input.className = "pair-path";
      input.value = path || "";
      const select = document.createElement("select");
      select.className = "pair-kind";
      (field.options || []).forEach((pair) => {
        const option = document.createElement("option");
        option.value = Array.isArray(pair) ? pair[0] : pair.value;
        option.textContent = Array.isArray(pair) ? pair[1] : pair.label;
        select.appendChild(option);
      });
      select.value = kind || (field.options && field.options[0] && field.options[0][0]) || "";
      row.append(input, select);
      container.appendChild(row);
    };
    paths.forEach((path, index) => addRow(path, kinds[index]));
    const add = document.createElement("button");
    add.type = "button";
    add.className = "ghost";
    add.textContent = "Add another folder";
    add.addEventListener("click", () => addRow("", ""));
    wrap.appendChild(container);
    wrap.appendChild(add);
  } else if (field.kind === "repeated") {
    const container = document.createElement("div");
    container.id = fieldElementId(field.key);
    container.className = "repeat-list";
    const values = Array.isArray(value) ? value.slice() : value ? [String(value)] : [""];
    let addRow = (initial) => {
      const row = document.createElement("div");
      row.className = "repeat-row";
      const input = document.createElement("input");
      input.type = "text";
      input.value = initial || "";
      row.appendChild(input);
      container.appendChild(row);
    };
    values.forEach(addRow);
    const add = document.createElement("button");
    add.type = "button";
    add.className = "ghost";
    add.textContent = "Add another folder";
    add.addEventListener("click", () => addRow(""));
    wrap.appendChild(container);
    wrap.appendChild(add);
  } else if (field.kind === "select") {
    const select = document.createElement("select");
    select.id = fieldElementId(field.key);
    (field.options || []).forEach((pair) => {
      const option = document.createElement("option");
      option.value = Array.isArray(pair) ? pair[0] : pair.value;
      option.textContent = Array.isArray(pair) ? pair[1] : pair.label;
      select.appendChild(option);
    });
    select.value = value || (field.options && field.options[0] && field.options[0][0]) || "";
    wrap.appendChild(select);
  } else {
    const input = document.createElement("input");
    input.type = "text";
    input.id = fieldElementId(field.key);
    input.value = value || "";
    input.required = !!field.required;
    wrap.appendChild(input);
  }
  return wrap;
}

// Render the selected source's fields into the connection form.
function renderConnectionFields(kind, config) {
  const source = librarySources.find((entry) => entry.type_id === kind) || {};
  const host = $("ms-fields");
  if (!host) return;
  host.textContent = "";
  (source.fields || []).forEach((field) => {
    if (field.kind === "action") return;
    host.appendChild(renderSourceField(field, config));
  });
}

function currentFormConfig() {
  const kind = $("ms-kind").value;
  const source = librarySources.find((entry) => entry.type_id === kind) || {};
  const config = { enabled: $("ms-enabled").checked };
  (source.fields || []).forEach((field) => {
    if (field.kind === "action") return;
    if (field.kind === "pair_list") {
      const container = $(fieldElementId(field.key));
      if (container) {
        const inputs = Array.from(container.querySelectorAll(".pair-path"));
        const selects = Array.from(container.querySelectorAll(".pair-kind"));
        config[field.key] = inputs.map((input) => input.value.trim());
        config[field.second_key] = selects.map((select) => select.value);
      } else {
        config[field.key] = [];
        config[field.second_key] = [];
      }
      return;
    }
    if (field.kind === "repeated") {
      const container = $(fieldElementId(field.key));
      config[field.key] = container
        ? Array.from(container.querySelectorAll("input"))
            .map((input) => input.value.trim())
            .filter(Boolean)
        : [];
      return;
    }
    if (field.kind === "remaps") {
      const el = $(fieldElementId(field.key));
      config[field.key] = el ? linesToRemaps(el.value) : {};
      return;
    }
    const el = $(fieldElementId(field.key));
    config[field.key] = el ? el.value.trim() : "";
  });
  return config;
}

function openConnectionForm(connection) {
  connectionEditingId = connection ? connection.id : null;
  $("ms-form-title").textContent = connection ? "Edit connection" : "Add connection";
  const config = (connection && connection.config) || {};
  const kind =
    connection
      ? connection.kind
      : $("ms-kind").value || (librarySources[0] && librarySources[0].type_id) || "";
  $("ms-kind").value = kind;
  $("ms-kind").disabled = !!connection;
  renderConnectionFields(kind, config);
  $("ms-enabled").checked = config.enabled !== false;
  $("ms-test").hidden = !connection;
  $("ms-cancel").hidden = !connection;
  $("ms-result").textContent = "";
  $("ms-form").scrollIntoView({ block: "nearest" });
  const first = $("ms-fields").querySelector("input, select");
  if (first) first.focus();
}

async function saveConnection(event) {
  event.preventDefault();
  if (savingConnection) return; // a disabled button still lets Enter resubmit
  savingConnection = true;
  const kind = $("ms-kind").value;
  const config = currentFormConfig();
  const save = $("ms-save");
  save.disabled = true;
  try {
    const response = await request(connectionEditingId ? `/api/connections/${connectionEditingId}` : "/api/connections", {
      method: connectionEditingId ? "PUT" : "POST",
      body: JSON.stringify(connectionEditingId ? { config } : { kind, config }),
    });
    const createdId = response && response.connection && response.connection.id;
    openConnectionForm(null);
    $("ms-result").textContent = "Saved.";
    await refreshLibraryConnections();
    renderLibraryTabs();
    if (createdId) {
      // Move the connection into its own tab under its source (Jellyfin,
      // Local Files, Past Tense News).
      history.pushState(null, "", `/webui/library/${kind}/${createdId}`);
      libraryPage = connPageId(kind, createdId);
    }
    renderCurrentLibraryPage();
  } catch (error) {
    $("ms-result").textContent = error.message;
  } finally {
    savingConnection = false;
    save.disabled = false;
  }
}

async function testConnectionForm() {
  if (!connectionEditingId) return;
  try {
    await request(`/api/connections/${connectionEditingId}`, { method: "PUT", body: JSON.stringify({ config: currentFormConfig() }) });
    const data = await request(`/api/connections/${connectionEditingId}/test`, { method: "POST" });
    const result = data.result || {};
    $("ms-result").textContent = `${result.ok ? "OK" : "Failed"}: ${result.detail || ""}`;
  } catch (error) {
    $("ms-result").textContent = error.message;
  }
}

{
  const form = $("ms-form");
  if (form) form.addEventListener("submit", saveConnection);
  const kindSelect = $("ms-kind");
  if (kindSelect) {
    // Switching source re-renders that source's own fields.
    kindSelect.addEventListener("change", () => {
      const kind = kindSelect.value;
      renderConnectionFields(kind, {});
      $("ms-test").hidden = !connectionEditingId;
      $("ms-cancel").hidden = !connectionEditingId;
    });
  }
  const test = $("ms-test");
  if (test) test.addEventListener("click", testConnectionForm);
  const cancel = $("ms-cancel");
  if (cancel) cancel.addEventListener("click", () => openConnectionForm(null));
}

CF.define("jellyfin", { onShow: loadLibrary });
