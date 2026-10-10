// People page: everyone synced from the library's cast lists, alphabetical.
const PEOPLE_ROUTE = "/api/plugins/com.channelflow.jellyfin/people";
let allPeople = [];

function peopleInitials(name) {
  return String(name)
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((word) => (word[0] || "").toUpperCase())
    .join("");
}

function peopleCard(person) {
  const url = person.image_path
    ? `/api/media/image?path=${encodeURIComponent(person.image_path)}`
    : null;
  const card = document.createElement("div");
  card.className = "people-card" + (url ? "" : " no-photo");
  const photo = url
    ? `<img class="people-photo" src="${escapeHtml(url)}" alt="${escapeHtml(person.name)}" loading="lazy" onerror="this.remove(); this.closest('.people-avatar').querySelector('.people-initial').style.display='flex';">`
    : "";
  card.innerHTML =
    `<div class="people-avatar">` +
      `<span class="people-initial">${escapeHtml(peopleInitials(person.name))}</span>` +
      photo +
    `</div>` +
    `<div class="people-name" title="${escapeHtml(person.name)}">${escapeHtml(person.name)}</div>`;
  return card;
}

function renderPeopleCircle() {
  const grid = $("people-grid");
  const empty = $("people-empty");
  const filter = $("people-filter") ? $("people-filter").value.trim().toLowerCase() : "";
  const count = $("people-count");
  if (count) count.textContent = `${allPeople.length} person(s)`;
  if (!grid) return;
  grid.textContent = "";
  const shown = filter ? allPeople.filter((person) => String(person.name).toLowerCase().includes(filter)) : allPeople;
  if (!shown.length) {
    if (empty) empty.hidden = false;
    return;
  }
  if (empty) empty.hidden = true;
  shown.forEach((person) => grid.appendChild(peopleCard(person)));
}

async function loadPeople() {
  try {
    const data = await request(PEOPLE_ROUTE);
    allPeople = (data.people || []).slice().sort((a, b) =>
      String(a.name).localeCompare(String(b.name), undefined, { sensitivity: "base" })
    );
  } catch (error) {
    allPeople = [];
  }
  renderPeopleCircle();
}

// Live filtering without another trip to the server.
document.addEventListener("input", (event) => {
  if (event.target && event.target.id === "people-filter") {
    renderPeopleCircle();
  }
});

CF.define("people", { onShow: loadPeople });