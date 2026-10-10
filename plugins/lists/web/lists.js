// Lists: named collections of Media-catalog items you reuse across channels
// and presets. The plugin stores them; items are picked from the base /api/media
// catalog (which the browser can read with the admin session).

const LISTS_ROUTE = "/api/plugins/com.channelflow.lists/lists";
let allLists = [];
let catalogItems = [];

async function loadLists() {
  try {
    const data = await request(LISTS_ROUTE);
    allLists = data.lists || [];
  } catch (error) {
    allLists = [];
    const host = $("lists-container");
    if (host) host.innerHTML = `<p class="hint bad">${escapeHtml(error.message)}</p>`;
  }
  renderLists();
}

function renderLists() {
  const host = $("lists-container");
  const count = $("lists-count");
  if (count) count.textContent = `${allLists.length} list(s)`;
  if (!host) return;
  host.textContent = "";
  if (!allLists.length) {
    host.innerHTML = '<p class="hint">No lists yet — create one and add items from the Media catalog.</p>';
    return;
  }
  allLists.forEach((list) => {
    const card = document.createElement("div");
    card.className = "list-card";
    const items = list.items || [];
    const head = document.createElement("div");
    head.className = "list-card-head";
    const rename = document.createElement("button");
    rename.type = "button";
    rename.className = "ghost";
    rename.textContent = "Rename";
    rename.addEventListener("click", () => promptRename(list));
    const addItems = document.createElement("button");
    addItems.type = "button";
    addItems.className = "ghost";
    addItems.textContent = "Add items";
    addItems.addEventListener("click", () => openPicker(list));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "ghost";
    remove.textContent = "Delete";
    remove.addEventListener("click", () => deleteList(list));
    head.innerHTML = `<strong>${escapeHtml(list.name)}</strong> <span class="count-label">${items.length} item(s)</span>`;
    head.appendChild(document.createElement("span")).className = "spacer";
    head.appendChild(rename);
    head.appendChild(addItems);
    head.appendChild(remove);
    card.appendChild(head);

    if (!items.length) {
      const empty = document.createElement("div");
      empty.className = "list-empty";
      empty.textContent = "No items yet.";
      card.appendChild(empty);
    } else {
      items.forEach((item) => {
        const row = document.createElement("div");
        row.className = "list-item-row";
        row.innerHTML =
          `<span class="kind">${escapeHtml(item.kind || "")}</span>` +
          `<span class="title" title="${escapeHtml(item.title)}">${escapeHtml(item.title)}</span>` +
          `${item.year ? `<span style="color:var(--muted)">${escapeHtml(String(item.year))}</span>` : ""}` +
          `<button type="button" class="ghost">Remove</button>`;
        row.querySelector("button").addEventListener("click", () => removeItem(list, item));
        card.appendChild(row);
      });
    }
    host.appendChild(card);
  });
}

async function createList(event) {
  event.preventDefault();
  const name = $("new-list-name").value.trim();
  const result = $("lists-result");
  if (!name) {
    if (result) result.textContent = "Enter a name first.";
    return;
  }
  try {
    await request(LISTS_ROUTE, { method: "POST", body: JSON.stringify({ name }) });
    $("new-list-name").value = "";
    if (result) result.textContent = "List created.";
    await loadLists();
  } catch (error) {
    if (result) result.textContent = error.message;
  }
}

function promptRename(list) {
  const name = window.prompt("List name", list.name || "");
  if (name === null || !name.trim()) return;
  request(LISTS_ROUTE + "/" + encodeURIComponent(list.id), {
    method: "PUT",
    body: JSON.stringify({ name: name.trim() }),
  })
    .then(loadLists)
    .catch((error) => showToast(error.message));
}

async function deleteList(list) {
  if (!window.confirm(`Delete the list "${list.name}"?`)) return;
  try {
    await request(LISTS_ROUTE + "/" + encodeURIComponent(list.id), { method: "DELETE" });
    await loadLists();
  } catch (error) {
    showToast(error.message);
  }
}

async function removeItem(list, item) {
  try {
    await request(
      `${LISTS_ROUTE}/${encodeURIComponent(list.id)}/items/${encodeURIComponent(item.match_key)}`,
      { method: "DELETE" }
    );
    await loadLists();
  } catch (error) {
    showToast(error.message);
  }
}

async function ensureCatalog() {
  if (catalogItems.length) return;
  try {
    const data = await request("/api/media");
    catalogItems = data.items || [];
  } catch (error) {
    catalogItems = [];
  }
}

function openPicker(list) {
  const dialog = $("list-items-dialog");
  if (!dialog) return;
  $("list-items-title").textContent = `Add items to ${list.name}`;
  $("list-items-body").textContent = "Loading…";
  ensureCatalog().then(() => renderPicker(list));
  dialog.showModal();
}

function renderPicker(list) {
  const body = $("list-items-body");
  if (!body) return;
  body.textContent = "";
  const tabs = document.createElement("div");
  tabs.className = "kind-tabs";
  const kinds = [
    ["movie", "Movies"],
    ["series", "TV Shows"],
    ["artist", "Artists"],
    ["album", "Albums"],
    ["musicvideo", "Music Videos"],
  ];
  let activeKind = "movie";
  const render = () => {
    body.querySelectorAll(".list-pick-row").forEach((row) => row.remove());
    const inList = new Set((list.items || []).map((item) => item.match_key));
    const shown = catalogItems.filter(
      (item) => item.kind === activeKind && !inList.has(item.match_key)
    );
    if (!shown.length) {
      const none = document.createElement("p");
      none.className = "list-empty";
      none.textContent =
        inList.size && !shown.length
          ? "All catalog items of this kind are already on the list."
          : "Nothing added yet to the catalog for this kind.";
      body.appendChild(none);
      return;
    }
    shown.slice(0, 200).forEach((item) => {
      const row = document.createElement("div");
      row.className = "list-pick-row";
      row.innerHTML =
        `<span class="title" title="${escapeHtml(item.title)}">${escapeHtml(item.title)}</span>` +
        `${item.year ? `<span style="color:var(--muted)">${escapeHtml(String(item.year))}</span>` : ""}` +
        `<button type="button" class="ghost">Add</button>`;
      row.querySelector("button").addEventListener("click", async () => {
        try {
          await request(`${LISTS_ROUTE}/${encodeURIComponent(list.id)}/items`, {
            method: "POST",
            body: JSON.stringify({
              match_key: item.match_key,
              kind: item.kind,
              title: item.title,
              year: item.year || null,
            }),
          });
          list.items = list.items || [];
          list.items.push({
            match_key: item.match_key,
            kind: item.kind,
            title: item.title,
            year: item.year || null,
          });
          render();
        } catch (error) {
          showToast(error.message);
        }
      });
      body.appendChild(row);
    });
  };
  kinds.forEach(([kind, label]) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ghost" + (kind === activeKind ? " active" : "");
    button.textContent = label;
    button.addEventListener("click", () => {
      activeKind = kind;
      tabs.querySelectorAll("button").forEach((other) => {
        other.classList.toggle("active", other === button);
      });
      render();
    });
    tabs.appendChild(button);
  });
  body.appendChild(tabs);
  render();
}

{
  const form = $("new-list-form");
  if (form) form.addEventListener("submit", createList);
  const close = $("list-items-close");
  if (close) close.addEventListener("click", () => $("list-items-dialog").close());
  const dialog = $("list-items-dialog");
  if (dialog) dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
}

CF.define("lists", { onShow: loadLists });