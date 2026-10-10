// Local Files: add folders on this machine as a media source. Browse the
// server's folders, pick one, say what's in it (movies/TV/music/music videos),
// and create the connection. A scan reads .nfo files + poster.jpg/png.

const LOCAL_ROUTE = "/api/plugins/com.channelflow.local";
const KIND_LABEL = { movies: "Movies", tv: "TV Shows", music: "Music", musicvideos: "Music Videos" };

async function browse(path) {
  const host = $("local-browser");
  if (!host) return;
  const params = new URLSearchParams();
  if (path) params.set("path", path);
  host.textContent = "Loading…";
  try {
    const data = await request(`${LOCAL_ROUTE}/browse?${params}`);
    if ($("local-path") && data.current) $("local-path").value = data.current;
    host.textContent = "";
    if (data.error) {
      host.innerHTML = `<p class="hint bad">${escapeHtml(data.error)}</p>`;
      return;
    }
    (data.entries || []).forEach((entry) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "dir";
      button.innerHTML = `<span class="name">${escapeHtml(entry.name)}</span> <span class="path">${escapeHtml(entry.path)}</span>`;
      button.addEventListener("click", () => browse(entry.path));
      host.appendChild(button);
    });
  } catch (error) {
    host.innerHTML = `<p class="hint bad">${escapeHtml(error.message)}</p>`;
  }
}

function localConnRow(connection) {
  const config = connection.config || {};
  const row = document.createElement("div");
  row.className = "local-conn";
  const kind = config.media_kind || "movies";
  row.innerHTML =
    `<div class="name">${escapeHtml(config.name || "Local")}</div>` +
    `<span class="kind">${escapeHtml(KIND_LABEL[kind] || kind)}</span>` +
    `<span class="path" title="${escapeHtml(config.url || "")}">${escapeHtml(config.url || "")}</span>` +
    `<span class="spacer"></span>` +
    `<button type="button" class="ghost local-sync">Sync</button>` +
    `<button type="button" class="ghost local-delete">Delete</button>`;
  const id = connection.id;
  row.querySelector(".local-sync").addEventListener("click", () => syncLocal(id));
  row.querySelector(".local-delete").addEventListener("click", async () => {
    if (!window.confirm("Delete this Local folder connection?")) return;
    try {
      await request(`/api/connections/${id}`, { method: "DELETE" });
      await loadLocalConnections();
    } catch (error) {
      showToast(error.message);
    }
  });
  return row;
}

async function loadLocalConnections() {
  const host = $("local-connections");
  const count = $("local-count");
  if (!host) return;
  try {
    const data = await request("/api/connections");
    const locals = (data.connections || []).filter((connection) => connection.kind === "local");
    if (count) count.textContent = `${locals.length} folder(s)`;
    host.textContent = "";
    if (!locals.length) {
      host.innerHTML = '<p class="hint">No Local folders yet — add one above.</p>';
      return;
    }
    locals.forEach((connection) => host.appendChild(localConnRow(connection)));
  } catch (error) {
    host.innerHTML = `<p class="hint bad">${escapeHtml(error.message)}</p>`;
  }
}

async function syncLocal(id) {
  showToast("Folder scan started in the background…");
  try {
    await request("/api/tasks/jellyfin-sync/run", {
      method: "POST",
      body: JSON.stringify({ connection_id: id }),
    });
    showToast("Scan started.");
  } catch (error) {
    showToast(error.message);
  }
}

async function createLocal(event) {
  event.preventDefault();
  const result = $("local-result");
  const name = $("local-name").value.trim();
  const path = $("local-path").value.trim();
  const kind = $("local-kind").value;
  if (!name || !path) {
    if (result) result.textContent = "Give the folder a name and pick a path.";
    return;
  }
  const button = $("local-create");
  if (button) button.disabled = true;
  try {
    const response = await request("/api/connections", {
      method: "POST",
      body: JSON.stringify({
        kind: "local",
        config: { name, url: path, media_kind: kind },
      }),
    });
    if (result) result.textContent = "Added. Run Sync to scan the folder into the Media catalog.";
    await loadLocalConnections();
  } catch (error) {
    if (result) result.textContent = error.message;
  } finally {
    if (button) button.disabled = false;
  }
}

{
  const form = $("local-form");
  if (form) form.addEventListener("submit", createLocal);
  const load = $("local-load");
  if (load) {
    load.addEventListener("click", () => browse($("local-path").value.trim()));
  }
  const first = $("local-path");
  if (first) browse(first.value || "/media");
}

CF.define("local", { onShow: loadLocalConnections });