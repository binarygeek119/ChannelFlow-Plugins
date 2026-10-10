// Past Tense News: point ChannelFlow at a folder of news events; each event
// folder becomes one Past Tense News item (TV-show-style, no seasons). The
// coverage videos inside are scanned into the database; playback comes later.

const PTN_ROUTE = "/api/plugins/com.channelflow.pasttense";

async function browse(path) {
  const host = $("ptn-browser");
  if (!host) return;
  const params = new URLSearchParams();
  if (path) params.set("path", path);
  host.textContent = "Loading…";
  try {
    const data = await request(`${PTN_ROUTE}/browse?${params}`);
    if ($("ptn-path") && data.current) $("ptn-path").value = data.current;
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

function connRow(connection) {
  const config = connection.config || {};
  const row = document.createElement("div");
  row.className = "ptn-conn";
  row.innerHTML =
    `<div class="name">${escapeHtml(config.name || "Past Tense News")}</div>` +
    `<span class="path" title="${escapeHtml(config.url || "")}">${escapeHtml(config.url || "")}</span>` +
    `<span class="spacer"></span>` +
    `<button type="button" class="ghost ptn-sync">Scan</button>` +
    `<button type="button" class="ghost ptn-delete">Delete</button>`;
  const id = connection.id;
  row.querySelector(".ptn-sync").addEventListener("click", async () => {
    showToast("Scan started in the background…");
    try {
      await request("/api/tasks/jellyfin-sync/run", {
        method: "POST",
        body: JSON.stringify({ connection_id: id }),
      });
    } catch (error) {
      showToast(error.message);
    }
  });
  row.querySelector(".ptn-delete").addEventListener("click", async () => {
    if (!window.confirm("Delete this news folder connection?")) return;
    try {
      await request(`/api/connections/${id}`, { method: "DELETE" });
      await loadConnections();
    } catch (error) {
      showToast(error.message);
    }
  });
  return row;
}

async function loadConnections() {
  const host = $("ptn-connections");
  const count = $("ptn-count");
  if (!host) return;
  try {
    const data = await request("/api/connections");
    const locals = (data.connections || []).filter((connection) => connection.kind === "news");
    if (count) count.textContent = `${locals.length} folder(s)`;
    host.textContent = "";
    if (!locals.length) {
      host.innerHTML = '<p class="hint">No news folders yet — add one above.</p>';
      return;
    }
    locals.forEach((connection) => host.appendChild(connRow(connection)));
  } catch (error) {
    host.innerHTML = `<p class="hint bad">${escapeHtml(error.message)}</p>`;
  }
}

async function createFolder(event) {
  event.preventDefault();
  const result = $("ptn-result");
  const name = $("ptn-name").value.trim();
  const path = $("ptn-path").value.trim();
  if (!path) {
    if (result) result.textContent = "Pick the events folder first.";
    return;
  }
  const button = $("ptn-create");
  if (button) button.disabled = true;
  try {
    await request("/api/connections", {
      method: "POST",
      body: JSON.stringify({
        kind: "news",
        config: { name: name || "Past Tense News", url: path },
      }),
    });
    if (result) result.textContent = "Added. Run Scan to read the event folders into the media database.";
    await loadConnections();
  } catch (error) {
    if (result) result.textContent = error.message;
  } finally {
    if (button) button.disabled = false;
  }
}

{
  const form = $("ptn-form");
  if (form) form.addEventListener("submit", createFolder);
  const load = $("ptn-load");
  if (load) load.addEventListener("click", () => browse($("ptn-path").value.trim()));
  const first = $("ptn-path");
  if (first) browse(first.value || "/media");
}

CF.define("pasttense", { onShow: loadConnections });