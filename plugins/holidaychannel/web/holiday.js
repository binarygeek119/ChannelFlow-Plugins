// The Holiday Channel page: the v1.0.0 seasonal calendar + the catalog lineup
// that matches the active holiday. The calendar and matching logic live in
// the plugin (Rust); this page fetches the catalog (only the logged-in front
// end can) and asks the plugin for the day's lineup.

const HOLIDAY_PLUGIN = "com.channelflow.holidaychannel";
const HOLIDAY_API = `/api/plugins/${HOLIDAY_PLUGIN}`;
const HOLIDAY_WEB = `/plugin/${HOLIDAY_PLUGIN}/web/`;
const HOLIDAY_CATALOG_KINDS = ["movie", "series"];

let holidayActive = null;
let holidayDate = todayString();

function todayString() {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function logoUrl(path) {
  return `${HOLIDAY_WEB}logos/${encodeURIComponent((path || "off-season.png").replace(/^logos\//, ""))}`;
}

async function loadHoliday() {
  renderCount("");
  try {
    const data = await request(`${HOLIDAY_API}/calendar?date=${encodeURIComponent(holidayDate)}`);
    renderNow(data);
    renderCalendar(data);
  } catch (error) {
    renderCount(error.message);
  }
  await loadLineup();
}

function renderCount(text) {
  const count = $("holiday-count");
  if (count) count.textContent = text;
}

function renderNow(data) {
  const now = $("holiday-now");
  if (!now) return;
  const active = data.active;
  const upcoming = data.upcoming;
  const logo = logoUrl(data.logo || "off-season.png");
  if (!active) {
    now.innerHTML =
      `<div class="holiday-card holiday-now-inner">` +
      `<img class="holiday-logo" src="${escapeHtml(logo)}" alt="" loading="lazy">` +
      `<div class="holiday-now-text">` +
      `<strong>Off season</strong>` +
      (upcoming
        ? `<p class="hint">Next up: ${escapeHtml(upcoming.name)} — ${escapeHtml(upcoming.window_start)} → ${escapeHtml(upcoming.window_end)}.</p>`
        : `<p class="hint">No holiday within its window right now.</p>`) +
      `</div></div>`;
    return;
  }
  now.innerHTML =
    `<div class="holiday-card holiday-now-inner">` +
    `<img class="holiday-logo" src="${escapeHtml(logo)}" alt="${escapeHtml(active.name)}" loading="lazy">` +
    `<div class="holiday-now-text">` +
    `<div class="holiday-kicker">Today on The Holiday Channel</div>` +
    `<h2>${escapeHtml(active.name)}</h2>` +
    `<p class="hint">Season ${escapeHtml(active.window_start)} → ${escapeHtml(active.window_end)} · observed ${escapeHtml(active.observed)}.</p>` +
    `</div></div>`;
  holidayActive = active.id;
}

function renderCalendar(data) {
  const box = $("holiday-calendar");
  if (!box) return;
  const holidays = data.holidays || [];
  if (!holidays.length) {
    box.innerHTML = "";
    return;
  }
  box.innerHTML =
    `<h3 class="holiday-section">The year on The Holiday Channel</h3>` +
    `<div class="holiday-year-grid">` +
    holidays
      .map((holiday) => {
        const y = holiday.this_year || {};
        const active = holidayActive === holiday.id;
        return `<div class="holiday-year-card ${active ? "holiday-active" : ""}">` +
          `<img class="holiday-year-logo" src="${escapeHtml(logoUrl(y.logo))}" alt="" loading="lazy">` +
          `<div class="holiday-year-name">${escapeHtml(holiday.name)}</div>` +
          `<div class="hint">${escapeHtml(y.window_start || "")} → ${escapeHtml(y.window_end || "")}</div>` +
          `</div>`;
      })
      .join("") +
    `</div>`;
}

async function fetchCatalogItems() {
  const all = [];
  const kinds = HOLIDAY_CATALOG_KINDS.join(",");
  let offset = 0;
  const page = 200;
  let total = null;
  for (;;) {
    const data = await request(
      `/api/media?kinds=${encodeURIComponent(kinds)}&offset=${offset}&limit=${page}`
    );
    all.push(...(data.items || []));
    total = data.total != null ? Number(data.total) : all.length;
    offset += page;
    if (offset >= total || (data.items || []).length === 0) break;
  }
  return all;
}

async function loadLineup() {
  const lineup = $("holiday-lineup");
  if (!lineup) return;
  lineup.innerHTML = '<p class="hint">Loading the catalog…</p>';
  let items = [];
  try {
    items = await fetchCatalogItems();
  } catch (error) {
    lineup.innerHTML = `<p class="hint bad">Could not load the catalog: ${escapeHtml(error.message)}</p>`;
    return;
  }
  let data;
  try {
    data = await request(`${HOLIDAY_API}/lineup`, {
      method: "POST",
      body: JSON.stringify({ date: holidayDate, items }),
    });
  } catch (error) {
    lineup.innerHTML = `<p class="hint bad">Could not build the lineup: ${escapeHtml(error.message)}</p>`;
    return;
  }
  renderCount(`${items.length} catalog items · ${(data.matches || []).length} in today's lineup`);
  lineup.textContent = "";
  if (data.off_season) {
    if (!data.matches || !data.matches.length) {
      lineup.innerHTML =
        `<div class="holiday-card"><div class="holiday-now-text">` +
        `<strong>Off season</strong>` +
        `<p class="hint">No holiday is active today, so The Holiday Channel rests on ${escapeHtml(data.off_season_media || "The Holiday Channel.mkv")}.</p>` +
        `</div></div>`;
    }
    return;
  }
  const matches = data.matches || [];
  if (!matches.length) {
    lineup.innerHTML =
      '<p class="hint">No catalog titles match the active holiday\'s keywords today.</p>';
    return;
  }
  const grid = document.createElement("div");
  grid.className = "holiday-lineup-grid";
  matches.forEach((match) => grid.appendChild(holidayCard(match)));
  lineup.appendChild(grid);
}

function holidayCard(item) {
  const tone = item.kind === "series" || item.kind === "movie" ? item.kind : "";
  const poster = item.poster_path
    ? `/api/media/image?path=${encodeURIComponent(item.poster_path)}`
    : null;
  const card = document.createElement("div");
  card.className = "holiday-card";
  card.innerHTML =
    (poster
      ? `<div class="holiday-poster" style="background-image:url('${escapeHtml(poster)}')"></div>`
      : `<div class="holiday-poster holiday-poster-fallback"></div>`) +
    `<div class="holiday-card-title" title="${escapeHtml(item.title || "")}">${escapeHtml(item.title || "")}</div>` +
    ((item.year || tone) ? `<div class="hint">${escapeHtml([tone, item.year].filter(Boolean).join(" · "))}</div>` : "");
  return card;
}

CF.define("holidaychannel", { onShow: loadHoliday });