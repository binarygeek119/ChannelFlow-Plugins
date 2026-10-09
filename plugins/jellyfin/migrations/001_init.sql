-- The Jellyfin Media Source schema.
--
-- This file is the authoritative shape of the plugin's database. At runtime
-- the plugin creates these tables (prefixed `cf_com_channelflow_jellyfin_`)
-- through the SDK's PluginDatabase::create_table on the base's Postgres; the
-- file serves as the migration/upgrade source of truth and for review.
--
-- `connections` is the base's shared media-source table; deleting a row there
-- cascades through item_sources because of the foreign keys below.

-- canonical items — deduped by title:year:type
CREATE TABLE IF NOT EXISTS media_items (
  id                    INTEGER PRIMARY KEY,
  dedup_key             TEXT NOT NULL UNIQUE,
  media_type            TEXT NOT NULL,
  title                 TEXT,
  sort_title            TEXT,
  original_title        TEXT,
  overview              TEXT,
  tagline               TEXT,
  container             TEXT,
  runtime_ticks         BIGINT,
  release_date          TEXT,
  year                  INTEGER,
  community_rating      REAL,
  critics_rating        REAL,
  official_rating       TEXT,
  custom_rating         TEXT,
  original_aspect_ratio TEXT,
  language              TEXT,
  original_language     TEXT,
  poster_path           TEXT,
  synced_at             TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_items_type ON media_items(media_type);

CREATE TABLE IF NOT EXISTS item_sources (
  id               INTEGER PRIMARY KEY,
  item_id          INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  connection_id    INTEGER NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
  jellyfin_id      TEXT NOT NULL,
  path             TEXT,
  local_path       TEXT,
  is_missing       INTEGER NOT NULL DEFAULT 0,
  last_scanned_at  TEXT,
  etag             TEXT,
  date_created     TEXT,
  date_last_saved  TEXT,
  synced_at        TEXT NOT NULL,
  UNIQUE(connection_id, jellyfin_id)
);
CREATE INDEX IF NOT EXISTS idx_sources_item ON item_sources(item_id);
CREATE INDEX IF NOT EXISTS idx_sources_missing ON item_sources(is_missing);

CREATE TABLE IF NOT EXISTS media_files (
  id            INTEGER PRIMARY KEY,
  source_id     INTEGER NOT NULL REFERENCES item_sources(id) ON DELETE CASCADE,
  remote_id     TEXT,
  container     TEXT,
  path          TEXT,
  local_path    TEXT,
  size_bytes    BIGINT,
  bitrate       INTEGER,
  runtime_ticks BIGINT,
  is_remote     INTEGER DEFAULT 0,
  protocol      TEXT,
  height        INTEGER,
  video_codec   TEXT,
  hdr_format    TEXT
);

CREATE TABLE IF NOT EXISTS video_streams (
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  stream_index INTEGER, codec TEXT, profile TEXT, level TEXT, resolution TEXT, aspect_ratio TEXT,
  is_interlaced INTEGER, is_anamorphic INTEGER, bitrate INTEGER, framerate REAL, bit_depth INTEGER,
  video_range TEXT, video_range_type TEXT, pixel_format TEXT, ref_frames INTEGER,
  dv_profile INTEGER, dv_level INTEGER, dv_bl_signal_compatibility_id INTEGER);

CREATE TABLE IF NOT EXISTS audio_streams (
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  stream_index INTEGER, title TEXT, language TEXT, codec TEXT, profile TEXT, layout TEXT,
  channels INTEGER, bitrate INTEGER, sample_rate INTEGER,
  is_default INTEGER, is_forced INTEGER, is_external INTEGER, loudness_lufs REAL);

CREATE TABLE IF NOT EXISTS subtitle_streams (
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  stream_index INTEGER, title TEXT, language TEXT, codec TEXT,
  is_default INTEGER, is_forced INTEGER, is_external INTEGER, external_path TEXT);

CREATE TABLE IF NOT EXISTS chapters (
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  start_ticks BIGINT, name TEXT, chapter_type TEXT);

CREATE TABLE IF NOT EXISTS people (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, image_path TEXT);
CREATE TABLE IF NOT EXISTS item_people (
  item_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  role_type TEXT, role TEXT, character TEXT, sort_order INTEGER,
  PRIMARY KEY(item_id, person_id, role_type, role));

CREATE TABLE IF NOT EXISTS genres  (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS studios (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS tags    (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS item_genres  (item_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE, genre_id INTEGER REFERENCES genres(id) ON DELETE CASCADE, PRIMARY KEY(item_id, genre_id));
CREATE TABLE IF NOT EXISTS item_studios (item_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE, studio_id INTEGER REFERENCES studios(id) ON DELETE CASCADE, PRIMARY KEY(item_id, studio_id));
CREATE TABLE IF NOT EXISTS item_tags    (item_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE, tag_id INTEGER REFERENCES tags(id) ON DELETE CASCADE, PRIMARY KEY(item_id, tag_id));

CREATE TABLE IF NOT EXISTS provider_ids (
  id INTEGER PRIMARY KEY, item_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  provider TEXT NOT NULL, value TEXT NOT NULL, UNIQUE(item_id, provider));

CREATE TABLE IF NOT EXISTS movies   (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS series   (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE, status TEXT, air_days TEXT, total_seasons INTEGER, total_episodes INTEGER);
CREATE TABLE IF NOT EXISTS seasons  (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE, series_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE, season_number INTEGER);
CREATE TABLE IF NOT EXISTS episodes (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE, series_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE, season_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE, season_number INTEGER, episode_number INTEGER);
CREATE TABLE IF NOT EXISTS artists  (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS albums   (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE, artist_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS tracks   (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE, album_id INTEGER REFERENCES media_items(id) ON DELETE CASCADE, track_number INTEGER, disc_number INTEGER);
CREATE TABLE IF NOT EXISTS music_videos (item_id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE, artist_id INTEGER REFERENCES media_items(id) ON DELETE SET NULL);

CREATE TABLE IF NOT EXISTS sync_runs (
  id INTEGER PRIMARY KEY, connection_id INTEGER NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
  library_id INTEGER, kind TEXT NOT NULL, status TEXT NOT NULL, started_at TEXT NOT NULL,
  finished_at TEXT, items_added INTEGER DEFAULT 0, items_updated INTEGER DEFAULT 0,
  items_removed INTEGER DEFAULT 0, errors INTEGER DEFAULT 0, message TEXT);

CREATE TABLE IF NOT EXISTS playout_pins (
  id INTEGER PRIMARY KEY, item_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  window_start TEXT, created_at TEXT NOT NULL);