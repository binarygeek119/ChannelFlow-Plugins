// People page: everyone synced from the library's cast lists, alphabetical.
// Clicking a person opens /webui/people/<name> — a page like a media item's,
// listing everything that person appears in. Clicking one of those opens the
// item's Media-page detail.
const PEOPLE_ROUTE = "/api/plugins/com.channelflow.jellyfin/people";
const MEDIA_ITEM_BASE = "/webui/media/item/";
let allPeople = [];
let sourceIndex = null;

const PEOPLE_KIND_LABEL = {
  movie: "Movie",
  series: "Series",
  album: "Album",
  artist: "Artist",
  musicvideo: "Music video",
};

function peopleInitials(name) {
  return String(name)
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((word) => (word[0] || "").toUpperCase())
    .join("");
}

function peoplePhotoUrl(imagePath) {
  return imagePath ? `/api/media/image?path=${encodeURIComponent(imagePath)}` : null;
}

// Which person (if any) the current URL points at.
function personNameFromPath() {
  const match = location.pathname.match(/^\/webui\/people\/(.+)$/);
  return match ? decodeURIComponent(match[1]) : null;
}

function openPerson(name) {
  history.pushState(null, "", `/webui/people/${encodeURIComponent(name)}`);
  loadPeople();
}

function openPersonList() {
  history.pushState(null, "", "/webui/people");
  loadPeople();
}

function openMediaItem(matchKey) {
  history.pushState(null, "", MEDIA_ITEM_BASE + encodeURIComponent(matchKey));
  showTab("media");
}

// ── People list ───────────────────────────────────────────────────────────

function peopleCard(person) {
  const url = peoplePhotoUrl(person.image_path);
  const card = document.createElement("div");
  card.className = "people-card" + (url ? "" : " no-photo");
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.title = person.name;
  const photo = url
    ? `<img class="people-photo" src="${escapeHtml(url)}" alt="${escapeHtml(person.name)}" loading="lazy" onerror="this.remove(); this.closest('.people-avatar').querySelector('.people-initial').style.display='flex';">`
    : "";
  card.innerHTML =
    `<div class="people-avatar">` +
      `<span class="people-initial">${escapeHtml(peopleInitials(person.name))}</span>` +
      photo +
    `</div>` +
    `<div class="people-name" title="${escapeHtml(person.name)}">${escapeHtml(person.name)}</div>`;
  card.addEventListener("click", () => openPerson(person.name));
  card.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openPerson(person.name);
    }
  });
  return card;
}

function renderPeopleList() {
  showPeopleChrome(true);
  const heading = $("people-heading");
  if (heading) heading.textContent = "People";
  const grid = $("people-grid");
  const empty = $("people-empty");
  const filter = $("people-filter") ? $("people-filter").value.trim().toLowerCase() : "";
  const count = $("people-count");
  if (count) count.textContent = `${allPeople.length} person(s)`;
  if (!grid) return;
  grid.className = "people-grid";
  grid.textContent = "";
  const shown = filter
    ? allPeople.filter((person) => String(person.name).toLowerCase().includes(filter))
    : allPeople;
  if (!shown.length) {
    if (empty) empty.hidden = false;
    return;
  }
  if (empty) empty.hidden = true;
  shown.forEach((person) => grid.appendChild(peopleCard(person)));
}

// Toggle the list-only chrome (hint + filter) when a person page is open.
function showPeopleChrome(visible) {
  const toolbar = $("people-toolbar");
  if (toolbar) toolbar.hidden = !visible;
  const hint = $("people-hint");
  if (hint) hint.hidden = !visible;
  const empty = $("people-empty");
  if (empty && visible) empty.hidden = true;
}

// ── One person: their page, like a media item's ──────────────────────────

async function ensureSourceIndex() {
  if (sourceIndex) return sourceIndex;
  sourceIndex = {};
  try {
    const data = await request("/api/media/source-index");
    sourceIndex = data.index || {};
  } catch (error) {
    sourceIndex = {};
  }
  return sourceIndex;
}

function personItemCard(item, matchKey, posterPath) {
  const card = document.createElement("div");
  const clickable = Boolean(matchKey);
  card.className = "people-item" + (clickable ? "" : " unlinked");
  const url = peoplePhotoUrl(posterPath || item.poster_path);
  const poster = url
    ? `<div class="people-item-poster" style="background-image:url('${escapeHtml(url)}')"></div>`
    : `<div class="people-item-poster people-item-poster-fallback"><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><circle cx="10" cy="10" r="2"/><path d="M4 18l4.5-4.5 3 3L16 12l4 4"/></svg></div>`;
  const secondary = item.year
    ? String(item.year)
    : PEOPLE_KIND_LABEL[item.media_type] || item.media_type || "";
  card.innerHTML =
    poster +
    `<div class="people-item-title" title="${escapeHtml(item.title || "")}">${escapeHtml(item.title || "")}</div>` +
    (item.character
      ? `<div class="people-item-sub" title="${escapeHtml(item.character)}">${escapeHtml(item.character)}</div>`
      : `<div class="people-item-sub">${escapeHtml(secondary)}</div>`);
  if (clickable) {
    card.tabIndex = 0;
    card.setAttribute("role", "button");
    const go = () => openMediaItem(matchKey);
    card.addEventListener("click", go);
    card.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        go();
      }
    });
  }
  return card;
}

async function renderPerson(name) {
  showPeopleChrome(false);
  const heading = $("people-heading");
  if (heading) heading.textContent = name;
  const grid = $("people-grid");
  const empty = $("people-empty");
  if (empty) empty.hidden = true;
  if (!grid) return;
  grid.className = "people-person";
  grid.textContent = "";

  let person = null;
  try {
    const data = await request(`${PEOPLE_ROUTE}/${encodeURIComponent(name)}`);
    person = data.person || null;
  } catch (error) {
    person = null;
  }
  if (!person) {
    grid.appendChild(
      libraryCard('<p class="hint bad">That person is not in the catalog.</p>')
    );
    return;
  }

  const index = await ensureSourceIndex();

  const back = document.createElement("button");
  back.type = "button";
  back.className = "media-back";
  back.textContent = "← People";
  back.addEventListener("click", openPersonList);
  grid.appendChild(back);

  // Header: photo + name + how many titles.
  const header = document.createElement("div");
  header.className = "people-person-head";
  const url = peoplePhotoUrl(person.image_path);
  const photo = url
    ? `<img class="people-person-photo" src="${escapeHtml(url)}" alt="${escapeHtml(person.name)}" onerror="this.remove(); this.closest('.people-person-photo-frame').querySelector('.people-initial').style.display='flex';">`
    : "";
  header.innerHTML =
    `<div class="people-person-photo-frame">` +
      `<span class="people-initial">${escapeHtml(peopleInitials(person.name))}</span>` +
      photo +
    `</div>` +
    `<div class="people-person-meta">` +
      `<h1>${escapeHtml(person.name)}</h1>` +
      `<div class="people-person-sub">${(person.items || []).length} title(s) in your catalog</div>` +
    `</div>`;
  grid.appendChild(header);

  // The person's titles, each linking into the Media catalog when we can
  // resolve it there (dedup by the catalog item, not the source row).
  const seen = new Set();
  const titles = document.createElement("div");
  titles.className = "people-items";
  (person.items || []).forEach((item) => {
    const key = `jellyfin:${item.connection_id}:${item.jellyfin_id}`;
    const entry = index[key] || null;
    const matchKey = entry && entry.match_key ? entry.match_key : null;
    // The catalog item carries the poster (the plugin's own rows usually do
    // not), so borrow it when the plugin has none.
    const poster = (entry && entry.poster_path) || item.poster_path || null;
    const dedup = matchKey || `${item.media_type}:${item.title}:${item.year}`;
    if (seen.has(dedup)) return;
    seen.add(dedup);
    titles.appendChild(personItemCard(item, matchKey, poster));
  });
  if (!titles.children.length) {
    grid.appendChild(libraryCard('<p class="hint">No catalog titles for this person yet.</p>'));
    return;
  }
  grid.appendChild(titles);
}

async function loadPeople() {
  const personName = personNameFromPath();
  if (personName) {
    await renderPerson(personName);
    return;
  }
  if (!allPeople.length) {
    try {
      const data = await request(PEOPLE_ROUTE);
      allPeople = (data.people || []).slice().sort((a, b) =>
        String(a.name).localeCompare(String(b.name), undefined, { sensitivity: "base" })
      );
    } catch (error) {
      allPeople = [];
    }
  }
  renderPeopleList();
}

// Live filtering without another trip to the server.
document.addEventListener("input", (event) => {
  if (event.target && event.target.id === "people-filter") {
    renderPeopleList();
  }
});

CF.define("people", { onShow: loadPeople });