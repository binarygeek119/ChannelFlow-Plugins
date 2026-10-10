//! The Jellyfin plugin's database: its own tables in the base's Postgres,
//! created through the SDK with the `cf_com_channelflow_jellyfin_` prefix.
//! The shape mirrors `migrations/001_init.sql` (Postgres types); the `id` rows
//! are `BIGSERIAL` and foreign keys use the prefixed table names. The core's
//! `connections` table is referenced unprefixed — it is base-owned.

use std::sync::Arc;

use channelflow_plugin_api::database::{PluginDatabase, PluginDatabaseError};
use serde_json::Value;

pub type JfResult<T> = Result<T, PluginDatabaseError>;

/// The rich per-item metadata an upsert records (what an item detail page
/// shows). Built from one Jellyfin item payload; every field is optional so a
/// sparse response never fails the sync.
#[derive(Debug, Default, Clone)]
pub struct ItemDetail {
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub runtime_ticks: Option<i64>,
    pub release_date: Option<String>,
    pub community_rating: Option<f64>,
    pub critics_rating: Option<f64>,
    pub official_rating: Option<String>,
}

impl ItemDetail {
    pub fn from_raw(raw: &Value) -> Self {
        let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_string);
        Self {
            overview: text("Overview"),
            tagline: raw
                .get("Taglines")
                .and_then(Value::as_array)
                .and_then(|lines| lines.first())
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| text("Tagline")),
            sort_title: text("SortName"),
            original_title: text("OriginalTitle"),
            runtime_ticks: raw.get("RunTimeTicks").and_then(Value::as_i64),
            release_date: text("PremiereDate"),
            community_rating: raw.get("CommunityRating").and_then(Value::as_f64),
            critics_rating: raw.get("CriticRating").and_then(Value::as_f64),
            official_rating: text("OfficialRating"),
        }
    }
}

/// A thin wrapper over the base's plugin database handle.
pub struct MediaDb {
    inner: Arc<dyn PluginDatabase>,
}

impl MediaDb {
    pub fn new(inner: Arc<dyn PluginDatabase>) -> Self {
        Self { inner }
    }

    /// The fully-prefixed table name for `name`. On a backend with no plugin
/// database (the file store) there is no prefix; fall back to the plain name
/// so the caller can fail softly rather than panicking.
    pub fn t(&self, name: &str) -> String {
        self.inner.table_of(name).unwrap_or_else(|| name.to_string())
    }

    /// Create every table if it does not already exist.
    pub async fn init(&self) -> JfResult<()> {
        let sources = self.t("item_sources");
        let files = self.t("media_files");
        let people = self.t("people");
        let genres = self.t("genres");
        let studios = self.t("studios");
        let tags = self.t("tags");
        self.inner
            .create_table(
                "media_items",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     dedup_key TEXT NOT NULL,
                     media_type TEXT NOT NULL,
                     title TEXT, sort_title TEXT, original_title TEXT, overview TEXT,
                     tagline TEXT, container TEXT, runtime_ticks BIGINT, release_date TEXT,
                     year INTEGER, community_rating REAL, critics_rating REAL,
                     official_rating TEXT, custom_rating TEXT, original_aspect_ratio TEXT,
                     language TEXT, original_language TEXT, poster_path TEXT,
                     merged_into BIGINT,
                     synced_at TEXT NOT NULL"
                ),
            )
            .await?;
        // `dedup_key` is a *label*, not the identity: title:year:type collides
        // ("The Thing" 1982, twice), and merge/split are manual — so the key is
        // indexed but never unique. Existing databases made under the old
        // UNIQUE constraint get it dropped here.
        let items = self.t("media_items");
        self.inner
            .execute(&format!(
                "ALTER TABLE {items} DROP CONSTRAINT IF EXISTS {items}_dedup_key_key"
            ))
            .await?;
        self.inner
            .execute(&format!(
                "CREATE INDEX IF NOT EXISTS {items}_dedup_key_idx ON {items}(dedup_key)"
            ))
            .await?;
        self.inner
            .create_table(
                "item_sources",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     item_id BIGINT NOT NULL REFERENCES {items}(id) ON DELETE CASCADE,
                     connection_id INTEGER NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
                     jellyfin_id TEXT NOT NULL, path TEXT, local_path TEXT,
                     is_missing INTEGER NOT NULL DEFAULT 0, last_scanned_at TEXT, etag TEXT,
                     date_created TEXT, date_last_saved TEXT, synced_at TEXT NOT NULL,
                     UNIQUE(connection_id, jellyfin_id)"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "media_files",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     source_id BIGINT NOT NULL REFERENCES {sources}(id) ON DELETE CASCADE,
                     remote_id TEXT, container TEXT, path TEXT, local_path TEXT,
                     size_bytes BIGINT, bitrate INTEGER, runtime_ticks BIGINT,
                     is_remote INTEGER DEFAULT 0, protocol TEXT, height INTEGER,
                     video_codec TEXT, hdr_format TEXT"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "video_streams",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     file_id BIGINT NOT NULL REFERENCES {files}(id) ON DELETE CASCADE,
                     stream_index INTEGER, codec TEXT, profile TEXT, level TEXT,
                     resolution TEXT, aspect_ratio TEXT, is_interlaced INTEGER,
                     is_anamorphic INTEGER, bitrate INTEGER, framerate REAL, bit_depth INTEGER,
                     video_range TEXT, video_range_type TEXT, pixel_format TEXT,
                     ref_frames INTEGER, dv_profile INTEGER, dv_level INTEGER,
                     dv_bl_signal_compatibility_id INTEGER"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "audio_streams",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     file_id BIGINT NOT NULL REFERENCES {files}(id) ON DELETE CASCADE,
                     stream_index INTEGER, title TEXT, language TEXT, codec TEXT,
                     profile TEXT, layout TEXT, channels INTEGER, bitrate INTEGER,
                     sample_rate INTEGER, is_default INTEGER, is_forced INTEGER,
                     is_external INTEGER, loudness_lufs REAL"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "subtitle_streams",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     file_id BIGINT NOT NULL REFERENCES {files}(id) ON DELETE CASCADE,
                     stream_index INTEGER, title TEXT, language TEXT, codec TEXT,
                     is_default INTEGER, is_forced INTEGER, is_external INTEGER,
                     external_path TEXT"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "chapters",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     file_id BIGINT NOT NULL REFERENCES {files}(id) ON DELETE CASCADE,
                     start_ticks BIGINT, name TEXT, chapter_type TEXT"
                ),
            )
            .await?;
        self.inner
            .create_table("people", "id BIGSERIAL PRIMARY KEY, name TEXT NOT NULL UNIQUE, image_path TEXT")
            .await?;
        self.inner
            .create_table(
                "item_people",
                &format!(
                    "item_id BIGINT NOT NULL REFERENCES {items}(id) ON DELETE CASCADE,
                     person_id BIGINT NOT NULL REFERENCES {people}(id) ON DELETE CASCADE,
                     role_type TEXT, role TEXT, character TEXT, sort_order INTEGER,
                     PRIMARY KEY(item_id, person_id, role_type, role)"
                ),
            )
            .await?;
        for class in ["genres", "studios", "tags"] {
            self.inner
                .create_table(class, "id BIGSERIAL PRIMARY KEY, name TEXT NOT NULL UNIQUE")
                .await?;
        }
        self.inner
            .create_table(
                "item_genres",
                &format!(
                    "item_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE,
                     genre_id BIGINT REFERENCES {genres}(id) ON DELETE CASCADE,
                     PRIMARY KEY(item_id, genre_id)"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "item_studios",
                &format!(
                    "item_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE,
                     studio_id BIGINT REFERENCES {studios}(id) ON DELETE CASCADE,
                     PRIMARY KEY(item_id, studio_id)"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "item_tags",
                &format!(
                    "item_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE,
                     tag_id BIGINT REFERENCES {tags}(id) ON DELETE CASCADE,
                     PRIMARY KEY(item_id, tag_id)"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "provider_ids",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     item_id BIGINT NOT NULL REFERENCES {items}(id) ON DELETE CASCADE,
                     provider TEXT NOT NULL, value TEXT NOT NULL, UNIQUE(item_id, provider)"
                ),
            )
            .await?;
        self.inner
            .create_table("movies", &format!("item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE"))
            .await?;
        self.inner
            .create_table(
                "series",
                &format!(
                    "item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE,
                     status TEXT, air_days TEXT, total_seasons INTEGER, total_episodes INTEGER"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "seasons",
                &format!(
                    "item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE,
                     series_id BIGINT NOT NULL REFERENCES {items}(id) ON DELETE CASCADE,
                     season_number INTEGER"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "episodes",
                &format!(
                    "item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE,
                     series_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE,
                     season_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE,
                     season_number INTEGER, episode_number INTEGER"
                ),
            )
            .await?;
        self.inner
            .create_table("artists", &format!("item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE"))
            .await?;
        self.inner
            .create_table(
                "albums",
                &format!(
                    "item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE,
                     artist_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "tracks",
                &format!(
                    "item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE,
                     album_id BIGINT REFERENCES {items}(id) ON DELETE CASCADE,
                     track_number INTEGER, disc_number INTEGER"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "music_videos",
                &format!(
                    "item_id BIGINT PRIMARY KEY REFERENCES {items}(id) ON DELETE CASCADE,
                     artist_id BIGINT REFERENCES {items}(id) ON DELETE SET NULL"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "sync_runs",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     connection_id INTEGER NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
                     library_id INTEGER, kind TEXT NOT NULL, status TEXT NOT NULL,
                     started_at TEXT NOT NULL, finished_at TEXT,
                     items_added INTEGER DEFAULT 0, items_updated INTEGER DEFAULT 0,
                     items_removed INTEGER DEFAULT 0, errors INTEGER DEFAULT 0, message TEXT"
                ),
            )
            .await?;
        self.inner
            .create_table(
                "playout_pins",
                &format!(
                    "id BIGSERIAL PRIMARY KEY,
                     item_id BIGINT NOT NULL REFERENCES {items}(id) ON DELETE CASCADE,
                     file_id BIGINT NOT NULL REFERENCES {files}(id) ON DELETE CASCADE,
                     window_start TEXT, created_at TEXT NOT NULL"
                ),
            )
            .await?;
        Ok(())
    }

    /// Find or create the item for one `(connection, jellyfin_id)` source.
    /// Sync's identity is the source, so two "The Thing" (1982)s from two
    /// servers stay separate rows and the merge screen decides their fate.
    /// Returns `(id, created)`.
    pub async fn upsert_item_by_source(
        &self,
        connection_id: i64,
        jellyfin_id: &str,
        dedup_key: &str,
        media_type: &str,
        title: &str,
        year: Option<i32>,
        poster_path: Option<&str>,
        detail: &ItemDetail,
        synced_at: &str,
    ) -> JfResult<(i64, bool)> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        let opt_text = |value: &Option<String>| {
            value
                .as_deref()
                .map(|text| Value::String(text.to_string()))
                .unwrap_or(Value::Null)
        };
        let opt_real = |value: &Option<f64>| value.map(Value::from).unwrap_or(Value::Null);
        let opt_int = |value: &Option<i64>| value.map(Value::from).unwrap_or(Value::Null);
        let rows = self
            .inner
            .fetch_params(
                &format!(
                    "SELECT item_id FROM {sources} \
                     WHERE connection_id = $1::integer AND jellyfin_id = $2"
                ),
                &[Value::from(connection_id), Value::String(jellyfin_id.to_string())],
            )
            .await?;
        if let Some(row) = rows.first() {
            let id = row["item_id"].as_i64().unwrap_or(0);
            self.inner
                .execute_params(
                    &format!(
                        "UPDATE {items} SET dedup_key = $1, media_type = $2, title = $3, \
                         year = $4::integer, poster_path = $5, overview = $6, tagline = $7, \
                         sort_title = $8, original_title = $9, runtime_ticks = $10::bigint, \
                         release_date = $11, community_rating = $12::double precision, \
                         critics_rating = $13::double precision, official_rating = $14, \
                         synced_at = $15 WHERE id = $16::bigint"
                    ),
                    &[
                        Value::String(dedup_key.to_string()),
                        Value::String(media_type.to_string()),
                        Value::String(title.to_string()),
                        year.map(Value::from).unwrap_or(Value::Null),
                        poster_path.map(|text| Value::String(text.to_string())).unwrap_or(Value::Null),
                        opt_text(&detail.overview),
                        opt_text(&detail.tagline),
                        opt_text(&detail.sort_title),
                        opt_text(&detail.original_title),
                        opt_int(&detail.runtime_ticks),
                        opt_text(&detail.release_date),
                        opt_real(&detail.community_rating),
                        opt_real(&detail.critics_rating),
                        opt_text(&detail.official_rating),
                        Value::String(synced_at.to_string()),
                        Value::from(id),
                    ],
                )
                .await?;
            Ok((id, false))
        } else {
            let rows = self
                .inner
                .fetch_params(
                    &format!(
                        "INSERT INTO {items} \
                         (dedup_key, media_type, title, year, poster_path, overview, tagline, \
                          sort_title, original_title, runtime_ticks, release_date, \
                          community_rating, critics_rating, official_rating, synced_at) \
                         VALUES ($1, $2, $3, $4::integer, $5, $6, $7, $8, $9, $10::bigint, \
                                 $11, $12::double precision, $13::double precision, $14, $15) \
                         RETURNING id"
                    ),
                    &[
                        Value::String(dedup_key.to_string()),
                        Value::String(media_type.to_string()),
                        Value::String(title.to_string()),
                        year.map(Value::from).unwrap_or(Value::Null),
                        poster_path.map(|text| Value::String(text.to_string())).unwrap_or(Value::Null),
                        opt_text(&detail.overview),
                        opt_text(&detail.tagline),
                        opt_text(&detail.sort_title),
                        opt_text(&detail.original_title),
                        opt_int(&detail.runtime_ticks),
                        opt_text(&detail.release_date),
                        opt_real(&detail.community_rating),
                        opt_real(&detail.critics_rating),
                        opt_text(&detail.official_rating),
                        Value::String(synced_at.to_string()),
                    ],
                )
                .await?;
            let id = rows
                .first()
                .and_then(|row| row["id"].as_i64())
                .unwrap_or(0);
            Ok((id, true))
        }
    }

    /// Replace an item's genre and studio links from the Jellyfin payload. The
    /// payload lists genres as strings and studios as `{ Name }` objects.
    pub async fn set_genres_and_studios(&self, item_id: i64, raw: &Value) -> JfResult<()> {
        self.link_named(item_id, raw, "Genres", "genres", "item_genres", "genre_id")
            .await?;
        self.link_named(item_id, raw, "Studios", "studios", "item_studios", "studio_id")
            .await?;
        Ok(())
    }

    async fn link_named(
        &self,
        item_id: i64,
        raw: &Value,
        key: &str,
        table: &str,
        link_table: &str,
        link_column: &str,
    ) -> JfResult<()> {
        let names: Vec<String> = raw
            .get(key)
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| entry.get("Name").and_then(Value::as_str).map(str::to_string))
                    })
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let class = self.t(table);
        let links = self.t(link_table);
        // The current set wins: drop old links, then re-add.
        self.inner
            .execute_params(
                &format!("DELETE FROM {links} WHERE item_id = $1::bigint"),
                &[Value::from(item_id)],
            )
            .await?;
        for name in names {
            self.inner
                .execute_params(
                    &format!("INSERT INTO {class} (name) VALUES ($1) ON CONFLICT (name) DO NOTHING"),
                    &[Value::String(name.clone())],
                )
                .await?;
            let rows = self
                .inner
                .fetch_params(
                    &format!("SELECT id FROM {class} WHERE name = $1"),
                    &[Value::String(name)],
                )
                .await?;
            let Some(class_id) = rows.first().and_then(|row| row["id"].as_i64()) else {
                continue;
            };
            self.inner
                .execute_params(
                    &format!(
                        "INSERT INTO {links} (item_id, {link_column}) VALUES ($1::bigint, $2::bigint) \
                         ON CONFLICT DO NOTHING"
                    ),
                    &[Value::from(item_id), Value::from(class_id)],
                )
                .await?;
        }
        Ok(())
    }

    /// Find or create a grouping node (album, artist) by its dedup key. These
    /// have no sources of their own, so first-match-wins keeps one album row
    /// per title:year across connections.
    pub async fn upsert_grouping(
        &self,
        dedup_key: &str,
        media_type: &str,
        title: &str,
        year: Option<i32>,
        synced_at: &str,
    ) -> JfResult<(i64, bool)> {
        let items = self.t("media_items");
        let rows = self
            .inner
            .fetch_params(
                &format!(
                    "SELECT id FROM {items} WHERE dedup_key = $1 AND media_type = $2 \
                     AND merged_into IS NULL ORDER BY id LIMIT 1"
                ),
                &[Value::String(dedup_key.to_string()), Value::String(media_type.to_string())],
            )
            .await?;
        if let Some(row) = rows.first() {
            let id = row["id"].as_i64().unwrap_or(0);
            Ok((id, false))
        } else {
            self.insert_item(dedup_key, media_type, title, year, None, synced_at)
                .await
        }
    }

    async fn insert_item(
        &self,
        dedup_key: &str,
        media_type: &str,
        title: &str,
        year: Option<i32>,
        poster_path: Option<&str>,
        synced_at: &str,
    ) -> JfResult<(i64, bool)> {
        let items = self.t("media_items");
        let rows = self
            .inner
            .fetch_params(
                &format!(
                    "INSERT INTO {items} \
                     (dedup_key, media_type, title, year, poster_path, synced_at) \
                     VALUES ($1, $2, $3, $4::integer, $5, $6) RETURNING id"
                ),
                &[
                    Value::String(dedup_key.to_string()),
                    Value::String(media_type.to_string()),
                    Value::String(title.to_string()),
                    year.map(|value| Value::from(value)).unwrap_or(Value::Null),
                    poster_path.map(|text| Value::String(text.to_string())).unwrap_or(Value::Null),
                    Value::String(synced_at.to_string()),
                ],
            )
            .await?;
        let id = rows.first().and_then(|row| row["id"].as_i64()).unwrap_or(0);
        Ok((id, true))
    }

    /// Find or create the per-connection source row for a Jellyfin item.
    pub async fn upsert_source(
        &self,
        item_id: i64,
        connection_id: i64,
        jellyfin_id: &str,
        path: Option<&str>,
        is_missing: bool,
        synced_at: &str,
    ) -> JfResult<i64> {
        let sources = self.t("item_sources");
        let rows = self
            .inner
            .fetch_params(
                &format!(
                    "INSERT INTO {sources} \
                     (item_id, connection_id, jellyfin_id, path, is_missing, synced_at) \
                     VALUES ($1::bigint, $2::integer, $3, $4, $5::int, $6) \
                     ON CONFLICT (connection_id, jellyfin_id) DO UPDATE SET \
                       item_id = EXCLUDED.item_id, path = EXCLUDED.path, \
                       is_missing = EXCLUDED.is_missing, synced_at = EXCLUDED.synced_at \
                     RETURNING id"
                ),
                &[
                    Value::from(item_id),
                    Value::from(connection_id),
                    Value::String(jellyfin_id.to_string()),
                    path.map(|text| Value::String(text.to_string())).unwrap_or(Value::Null),
                    Value::from(if is_missing { 1 } else { 0 }),
                    Value::String(synced_at.to_string()),
                ],
            )
            .await?;
        Ok(rows
            .first()
            .and_then(|row| row["id"].as_i64())
            .unwrap_or(0))
    }

    /// Phase 2: record a freshly-written poster on the item(s) this source
    /// reported. Keyed by the source's own jellyfin id so a re-walk never
    /// touches another item.
    pub async fn set_poster(
        &self,
        connection_id: i64,
        jellyfin_id: &str,
        poster_path: &str,
    ) -> JfResult<u64> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        self.inner
            .execute_params(
                &format!(
                    "UPDATE {items} SET poster_path = $1 WHERE id IN \
                     (SELECT item_id FROM {sources} \
                      WHERE connection_id = $2::integer AND jellyfin_id = $3)"
                ),
                &[
                    Value::String(poster_path.to_string()),
                    Value::from(connection_id),
                    Value::String(jellyfin_id.to_string()),
                ],
            )
            .await
    }

    /// Phase 1: record an item's cast — a row per person (by name) plus the
    /// item link with role/character/order. People images are fetched in
    /// phase 2 and stored on the same rows.
    pub async fn upsert_people(&self, item_id: i64, raw: &Value) -> JfResult<()> {
        let Some(people) = raw.get("People").and_then(Value::as_array) else {
            return Ok(());
        };
        let people_t = self.t("people");
        let links = self.t("item_people");
        for (index, person) in people.iter().enumerate() {
            let name = person.get("Name").and_then(Value::as_str).unwrap_or("").trim();
            if name.is_empty() {
                continue;
            }
            self.inner
                .execute_params(
                    &format!("INSERT INTO {people_t} (name) VALUES ($1) ON CONFLICT (name) DO NOTHING"),
                    &[Value::String(name.to_string())],
                )
                .await?;
            let row = self
                .inner
                .fetch_params(
                    &format!("SELECT id FROM {people_t} WHERE name = $1"),
                    &[Value::String(name.to_string())],
                )
                .await?;
            let Some(row) = row.first() else { continue };
            let Some(person_id) = row["id"].as_i64() else { continue };
            let role_type = person.get("Type").and_then(Value::as_str).unwrap_or("").to_string();
            let role = person.get("Role").and_then(Value::as_str).unwrap_or("").to_string();
            let sort_order = person
                .get("SortOrder")
                .and_then(Value::as_i64)
                .unwrap_or(index as i64);
            self.inner
                .execute_params(
                    &format!(
                        "INSERT INTO {links} (item_id, person_id, role_type, role, character, sort_order) \
                         VALUES ($1::bigint, $2::bigint, $3, $4, $4, $5::int) \
                         ON CONFLICT (item_id, person_id, role_type, role) DO NOTHING"
                    ),
                    &[
                        Value::from(item_id),
                        Value::from(person_id),
                        Value::String(role_type),
                        Value::String(role),
                        Value::from(sort_order),
                    ],
                )
                .await?;
        }
        Ok(())
    }

    /// Phase 2: record a freshly-written person image on that person's row.
    pub async fn set_people_image(&self, name: &str, image_path: &str) -> JfResult<u64> {
        let people = self.t("people");
        self.inner
            .execute_params(
                &format!("UPDATE {people} SET image_path = $1 WHERE name = $2"),
                &[
                    Value::String(image_path.to_string()),
                    Value::String(name.to_string()),
                ],
            )
            .await
    }

    /// Every synced person, alphabetical by name (case-insensitive), with the
    /// picture each has if one was fetched.
    pub async fn people_list(&self) -> JfResult<Vec<Value>> {
        let people = self.t("people");
        self.inner
            .fetch(&format!(
                "SELECT name, image_path FROM {people} ORDER BY lower(name) ASC, name ASC"
            ))
            .await
    }

    /// The rich row behind a Media-page item: the Jellyfin metadata, its
    /// genres, studios, and cast (in billing order) — what an item detail
    /// page shows. Looks the item up by its Jellyfin id.
    pub async fn get_item_detail(&self, jellyfin_id: &str) -> JfResult<Option<Value>> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        let item_genres = self.t("item_genres");
        let genres = self.t("genres");
        let item_studios = self.t("item_studios");
        let studios = self.t("studios");
        let item_people = self.t("item_people");
        let people = self.t("people");
        self.inner
            .fetch_params(
                &format!(
                    "SELECT m.id, m.media_type, m.title, m.year, m.overview, m.tagline, \
                            m.runtime_ticks, m.release_date, m.community_rating, m.critics_rating, \
                            m.official_rating, m.poster_path, \
                            COALESCE((SELECT json_agg(g.name ORDER BY g.name) \
                                      FROM {item_genres} ig JOIN {genres} g ON g.id = ig.genre_id \
                                      WHERE ig.item_id = m.id), '[]'::json) AS genres, \
                            COALESCE((SELECT json_agg(s.name ORDER BY s.name) \
                                      FROM {item_studios} ist JOIN {studios} s ON s.id = ist.studio_id \
                                      WHERE ist.item_id = m.id), '[]'::json) AS studios, \
                            COALESCE((SELECT json_agg(json_build_object('name', p.name, 'image_path', p.image_path, \
                                            'role', ip.role, 'character', ip.character) \
                                      ORDER BY ip.sort_order, p.name) \
                                      FROM {item_people} ip JOIN {people} p ON p.id = ip.person_id \
                                      WHERE ip.item_id = m.id), '[]'::json) AS people \
                     FROM {items} m \
                     WHERE m.id = (SELECT s.item_id FROM {sources} s WHERE s.jellyfin_id = $1 LIMIT 1)"
                ),
                &[Value::String(jellyfin_id.to_string())],
            )
            .await
            .map(|rows| rows.into_iter().next())
    }

    /// Replace a source's files (and their streams/chapters) from the raw
    /// Jellyfin media sources.
    pub async fn replace_files(&self, source_id: i64, raw: &Value) -> JfResult<u64> {
        let files = self.t("media_files");
        let video = self.t("video_streams");
        let audio = self.t("audio_streams");
        let subtitles = self.t("subtitle_streams");
        let chapters = self.t("chapters");
        let mut written = 0u64;
        // Remove what a previous sync saw for this source.
        let old: Vec<i64> = self
            .inner
            .fetch_params(
                &format!("SELECT id FROM {files} WHERE source_id = $1::bigint"),
                &[Value::from(source_id)],
            )
            .await?
            .iter()
            .filter_map(|row| row["id"].as_i64())
            .collect();
        for file_id in old {
            for table in [&video, &audio, &subtitles, &chapters] {
                self.inner
                    .execute_params(
                        &format!("DELETE FROM {table} WHERE file_id = $1::bigint"),
                        &[Value::from(file_id)],
                    )
                    .await?;
            }
            self.inner
                .execute_params(
                    &format!("DELETE FROM {files} WHERE id = $1::bigint"),
                    &[Value::from(file_id)],
                )
                .await?;
        }
        // The raw item's MediaSources each become one file.
        let sources = raw
            .get("MediaSources")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        for source in sources {
            let rows = self
                .inner
                .fetch_params(
                    &format!(
                        "INSERT INTO {files} (source_id, remote_id, path, container, size_bytes, \
                         bitrate, runtime_ticks, is_remote, protocol, height, video_codec, hdr_format) \
                         VALUES ($1::bigint, $2, $3, $4::text, $5::bigint, $6::bigint, $7::bigint, $8::int, $9::text, $10::integer, $11::text, $12::text) \
                         RETURNING id"
                    ),
                    &[
                        Value::from(source_id),
                        Value::String(jf(&source, "Id")),
                        Value::String(jf(&source, "Path")),
                        Value::String(jf(&source, "Container")),
                        jf_num(&source, "Size"),
                        jf_num(&source, "Bitrate"),
                        jf_num(&source, "RunTimeTicks"),
                        Value::from(if source.get("IsRemote").and_then(|v| v.as_bool()).unwrap_or(false) { 1 } else { 0 }),
                        Value::String(jf(&source, "Protocol")),
                        // Height is numeric; jf() would hand an empty string to
                        // an integer column when Jellyfin omits it.
                        Value::from(source.get("Height").and_then(|value| value.as_i64()).unwrap_or(0)),
                        Value::String(jf(&source, "VideoCodec")),
                        Value::String(jf(&source, "VideoRange")),
                    ],
                )
                .await?;
            if let Some(file_id) = rows.first().and_then(|row| row["id"].as_i64()) {
                written += 1;
                self.replace_streams(file_id, &source).await?;
            }
        }
        Ok(written)
    }

    async fn replace_streams(&self, file_id: i64, source: &Value) -> JfResult<()> {
        let video = self.t("video_streams");
        let audio = self.t("audio_streams");
        let subtitles = self.t("subtitle_streams");
        let media_streams = source
            .get("MediaStreams")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        for stream in media_streams {
            let kind = jf(&stream, "Type");
            let index = stream.get("Index").and_then(|value| value.as_i64());
            let title = jf(&stream, "Title");
            let language = jf(&stream, "Language");
            let codec = jf(&stream, "Codec");
            let bitrate = jf_num(&stream, "BitRate");
            match kind.as_str() {
                "Video" => {
                    let profile = jf(&stream, "Profile");
                    let width = stream.get("Width").and_then(|value| value.as_i64());
                    let height = stream.get("Height").and_then(|value| value.as_i64());
                    let resolution = format!(
                        "{}x{}",
                        width.unwrap_or(0),
                        height.unwrap_or(0)
                    );
                    let framerate = stream
                        .get("AverageFrameRate")
                        .or_else(|| stream.get("RealFrameRate"))
                        .and_then(|value| value.as_f64());
                    let video_range = jf(&stream, "VideoRange");
                    self.inner
                        .execute_params(
                            &format!(
                                "INSERT INTO {video} (file_id, stream_index, codec, profile, \
                                 resolution, bitrate, framerate, video_range) \
                                 VALUES ($1::bigint, $2::integer, $3::text, $4::text, $5, $6::bigint, $7::real, $8::text)"
                            ),
                            &[
                                Value::from(file_id),
                                index.map(|value| Value::from(value)).unwrap_or(Value::Null),
                                Value::String(codec),
                                Value::String(profile),
                                Value::String(resolution),
                                bitrate,
                                framerate.map(|value| Value::from(value)).unwrap_or(Value::Null),
                                Value::String(video_range),
                            ],
                        )
                        .await?;
                }
                "Audio" => {
                    let channels = stream.get("Channels").and_then(|value| value.as_i64());
                    let is_default = stream.get("IsDefault").and_then(|v| v.as_bool()).unwrap_or(false);
                    let is_forced = stream.get("IsForced").and_then(|v| v.as_bool()).unwrap_or(false);
                    let layout = jf(&stream, "Layout");
                    self.inner
                        .execute_params(
                            &format!(
                                "INSERT INTO {audio} (file_id, stream_index, title, language, \
                                 codec, layout, channels, bitrate, is_default, is_forced) \
                                 VALUES ($1::bigint, $2::integer, $3::text, $4::text, $5::text, $6::text, $7::integer, $8::bigint, $9::int, $10::int)"
                            ),
                            &[
                                Value::from(file_id),
                                index.map(|value| Value::from(value)).unwrap_or(Value::Null),
                                Value::String(title),
                                Value::String(language),
                                Value::String(codec),
                                Value::String(layout),
                                channels.map(|value| Value::from(value)).unwrap_or(Value::Null),
                                bitrate,
                                Value::from(if is_default { 1 } else { 0 }),
                                Value::from(if is_forced { 1 } else { 0 }),
                            ],
                        )
                        .await?;
                }
                _ => {
                    let is_external = stream.get("IsExternal").and_then(|v| v.as_bool()).unwrap_or(false);
                    self.inner
                        .execute_params(
                            &format!(
                                "INSERT INTO {subtitles} (file_id, stream_index, title, language, \
                                 codec, is_default, is_external, external_path) \
                                 VALUES ($1::bigint, $2::integer, $3::text, $4::text, $5::text, $6::int, $7::int, $8::text)"
                            ),
                            &[
                                Value::from(file_id),
                                index.map(|value| Value::from(value)).unwrap_or(Value::Null),
                                Value::String(title),
                                Value::String(language),
                                Value::String(codec),
                                Value::from(0),
                                Value::from(if is_external { 1 } else { 0 }),
                                Value::String(jf(&stream, "Path")),
                            ],
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Every item this connection no longer reports gets marked missing.
    pub async fn mark_absent_missing(&self, connection_id: i64) -> JfResult<u64> {
        let sources = self.t("item_sources");
        self.inner
            .execute_params(
                &format!(
                    "UPDATE {sources} SET is_missing = 1 \
                     WHERE connection_id = $1::integer AND is_missing = 0 \
                       AND jellyfin_id NOT IN (SELECT jellyfin_id FROM {sources} WHERE connection_id = $1::integer)"
                ),
                &[Value::from(connection_id)],
            )
            .await
    }

    /// Hard-delete items marked missing longer than `days` ago, then sweep
    /// any item that no longer has a source at all. The base's nightly task
    /// calls this.
    #[allow(dead_code)]
    pub async fn grace_sweep(&self, days: i64) -> JfResult<()> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        let doomed = self
            .inner
            .fetch_params(
                &format!(
                    "SELECT {sources}.item_id FROM {sources} \
                     WHERE {sources}.is_missing = 1 AND {sources}.synced_at < now() - ($1::text || ' days')::interval"
                ),
                &[Value::String(days.to_string())],
            )
            .await?;
        for row in doomed {
            if let Some(item_id) = row["item_id"].as_i64() {
                self.inner
                    .execute_params(
                        &format!("DELETE FROM {items} WHERE id = $1::bigint::bigint"),
                        &[Value::from(item_id)],
                    )
                    .await?;
            }
        }
        self.orphan_sweep().await.map(|_| ())
    }

    /// Delete items that have no sources left (connection deleted, or the
    /// grace sweep removed the last one), returning the poster paths that are
    /// now dead so the caller can remove the files. Synthesised grouping nodes
    /// (`album`, `artist`) are never sourced and stay — they hold the live
    /// tracks beneath them.
    #[allow(dead_code)]
    pub async fn orphan_sweep(&self) -> JfResult<Vec<String>> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        let doomed = self
            .inner
            .fetch_params(
                &format!(
                    "SELECT {items}.id, {items}.poster_path FROM {items} \
                     WHERE NOT EXISTS (SELECT 1 FROM {sources} WHERE {sources}.item_id = {items}.id) \
                       AND {items}.merged_into IS NULL \
                       AND {items}.media_type NOT IN ('album', 'artist')"
                ),
                &[],
            )
            .await?;
        let mut posters = Vec::new();
        for row in doomed {
            if let Some(path) = row["poster_path"].as_str() {
                posters.push(path.to_string());
            }
            if let Some(id) = row["id"].as_i64() {
                self.inner
                    .execute_params(
                        &format!("DELETE FROM {items} WHERE id = $1::bigint::bigint"),
                        &[Value::from(id)],
                    )
                    .await?;
            }
        }
        Ok(posters)
    }

    /// All files for an item through its sources, shaped for the file
    /// picker.
    pub async fn item_files(&self, item_id: i64) -> JfResult<Vec<Value>> {
        let sources = self.t("item_sources");
        let files = self.t("media_files");
        self.inner
            .fetch_params(
                &format!(
                    "SELECT f.id AS file_id, f.source_id AS source_id, s.connection_id AS connection_id, \
                            f.height AS height, f.video_codec AS video_codec, f.hdr_format AS hdr_format, \
                            f.is_remote AS is_remote, s.is_missing AS is_missing, s.path AS path \
                     FROM {files} f JOIN {sources} s ON s.id = f.source_id \
                     WHERE s.item_id = $1::bigint \
                     ORDER BY f.id"
                ),
                &[Value::from(item_id)],
            )
            .await
    }

    /// Track the artist's row in the `artists` table.
    #[allow(dead_code)]
    pub async fn ensure_artist_row(&self, artist_item_id: i64) -> JfResult<()> {
        let artists = self.t("artists");
        self.inner
            .execute_params(
                &format!(
                    "INSERT INTO {artists} (item_id) VALUES ($1::bigint) \
                     ON CONFLICT (item_id) DO NOTHING"
                ),
                &[Value::from(artist_item_id)],
            )
            .await
            .map(|_| ())
    }

    /// Create or update an album row, linking it to its artist.
    pub async fn ensure_album_row(
        &self,
        album_item_id: i64,
        artist_item_id: Option<i64>,
    ) -> JfResult<()> {
        let albums = self.t("albums");
        self.inner
            .execute_params(
                &format!(
                    "INSERT INTO {albums} (item_id, artist_id) VALUES ($1::bigint, $2::bigint) \
                     ON CONFLICT (item_id) DO UPDATE SET artist_id = EXCLUDED.artist_id"
                ),
                &[
                    Value::from(album_item_id),
                    artist_item_id
                        .map(|id| Value::from(id))
                        .unwrap_or(Value::Null),
                ],
            )
            .await
            .map(|_| ())
    }

    /// Link a track into its album and record its disc/track numbers.
    pub async fn link_track(
        &self,
        track_item_id: i64,
        album_item_id: i64,
        track_number: Option<i32>,
        disc_number: Option<i32>,
    ) -> JfResult<()> {
        let tracks = self.t("tracks");
        self.inner
            .execute_params(
                &format!(
                    "INSERT INTO {tracks} (item_id, album_id, track_number, disc_number) \
                     VALUES ($1::bigint, $2::bigint, $3::integer, $4::integer) \
                     ON CONFLICT (item_id) DO UPDATE SET \
                       album_id = EXCLUDED.album_id, \
                       track_number = EXCLUDED.track_number, \
                       disc_number = EXCLUDED.disc_number"
                ),
                &[
                    Value::from(track_item_id),
                    Value::from(album_item_id),
                    value_or_null(track_number),
                    value_or_null(disc_number),
                ],
            )
            .await
            .map(|_| ())
    }

    /// Link a music video to its artist.
    pub async fn link_music_video(
        &self,
        video_item_id: i64,
        artist_item_id: Option<i64>,
    ) -> JfResult<()> {
        let music_videos = self.t("music_videos");
        self.inner
            .execute_params(
                &format!(
                    "INSERT INTO {music_videos} (item_id, artist_id) VALUES ($1::bigint, $2::bigint) \
                     ON CONFLICT (item_id) DO UPDATE SET artist_id = EXCLUDED.artist_id"
                ),
                &[
                    Value::from(video_item_id),
                    artist_item_id
                        .map(|id| Value::from(id))
                        .unwrap_or(Value::Null),
                ],
            )
            .await
            .map(|_| ())
    }

    // ── manual merge / split ──────────────────────────────────────────────

    /// Items that collide on a dedup key and are still separate, shaped for
    /// the merge screen. Only real media (not the album/artist grouping
    /// nodes) are offered.
    pub async fn merge_candidates(&self) -> JfResult<Vec<Value>> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        self.inner
            .fetch_params(
                &format!(
                    "SELECT m.dedup_key AS dedup_key, m.media_type AS media_type, \
                            json_agg(json_build_object( \
                              'id', m.id, 'title', m.title, 'year', m.year, \
                              'poster_path', m.poster_path, 'synced_at', m.synced_at, \
                              'sources', (SELECT count(*) FROM {sources} s WHERE s.item_id = m.id) \
                            ) ORDER BY m.id) AS items \
                     FROM {items} m \
                     WHERE m.merged_into IS NULL AND m.media_type NOT IN ('album', 'artist') \
                     GROUP BY m.dedup_key, m.media_type \
                     HAVING count(*) > 1 OR \
                            bool_or((SELECT count(*) FROM {sources} s WHERE s.item_id = m.id) > 1) \
                     ORDER BY m.dedup_key"
                ),
                &[],
            )
            .await
    }

    /// Move every source (and, by cascade, their files/streams) from
    /// `from_id` into `to_id`, adopt the poster if the target lacks one, and
    /// keep the emptied row as hidden history (`merged_into`).
    pub async fn merge_items(&self, from_id: i64, to_id: i64) -> JfResult<()> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        if from_id == to_id {
            return Ok(());
        }
        if self
            .inner
            .fetch_params(
                &format!(
                    "SELECT id FROM {items} WHERE id = $1::bigint AND merged_into IS NULL LIMIT 1"
                ),
                &[Value::from(to_id)],
            )
            .await?
            .is_empty()
        {
            return Err(PluginDatabaseError("the target item is not current".to_string()));
        }
        self.inner
            .execute_params(
                &format!("UPDATE {sources} SET item_id = $1::bigint WHERE item_id = $2::bigint"),
                &[Value::from(to_id), Value::from(from_id)],
            )
            .await?;
        self.inner
            .execute_params(
                &format!(
                    "UPDATE {items} SET poster_path = sub.poster_path FROM \
                     (SELECT poster_path FROM {items} WHERE id = $1::bigint) sub \
                     WHERE {items}.id = $2::bigint AND {items}.poster_path IS NULL \
                       AND sub.poster_path IS NOT NULL"
                ),
                &[Value::from(from_id), Value::from(to_id)],
            )
            .await?;
        self.inner
            .execute_params(
                &format!(
                    "UPDATE {items} SET merged_into = $1::bigint, synced_at = $2 WHERE id = $3::bigint"
                ),
                &[Value::from(to_id), Value::String(chrono::Utc::now().to_rfc3339()), Value::from(from_id)],
            )
            .await?;
        Ok(())
    }

    /// Break an item with several sources into one row per source. The first
    /// source keeps the original row; each remaining source gets a fresh row
    /// with a suffixed dedup key (it is a label, not the identity). Returns
    /// how many rows were created.
    pub async fn split_item(&self, item_id: i64) -> JfResult<usize> {
        let items = self.t("media_items");
        let sources = self.t("item_sources");
        let rows = self
            .inner
            .fetch_params(
                &format!(
                    "SELECT id, jellyfin_id FROM {sources} \
                     WHERE item_id = $1::bigint ORDER BY id",
                ),
                &[Value::from(item_id)],
            )
            .await?;
        let mut made = 0usize;
        for (index, row) in rows.iter().enumerate() {
            if index == 0 {
                continue; // first source keeps the original row
            }
            let source_id = row["id"].as_i64().unwrap_or(0);
            let jellyfin_id = row["jellyfin_id"].as_str().unwrap_or("source");
            let detail = self
                .inner
                .fetch_params(
                    &format!(
                        "SELECT dedup_key, media_type, title, year, poster_path, synced_at \
                         FROM {items} WHERE id = $1::bigint",
                    ),
                    &[Value::from(item_id)],
                )
                .await?;
            let Some(base) = detail.first() else { continue };
            let dedup_key = base["dedup_key"].as_str().unwrap_or("item");
            let suffix: String = jellyfin_id
                .chars()
                .take(8)
                .collect();
            let label = format!("{dedup_key}#{suffix}");
            let (new_item, _) = self
                .insert_item(
                    &label,
                    base["media_type"].as_str().unwrap_or("movie"),
                    base["title"].as_str().unwrap_or(""),
                    base["year"].as_i64().map(|year| year as i32),
                    None,
                    base["synced_at"].as_str().unwrap_or(""),
                )
                .await?;
            self.inner
                .execute_params(
                    &format!("UPDATE {sources} SET item_id = $1::bigint WHERE id = $2::bigint"),
                    &[Value::from(new_item), Value::from(source_id)],
                )
                .await?;
            made += 1;
        }
        Ok(made)
    }

    /// Remember a playout pin: use `file_id` for this item's window.
    pub async fn save_pin(&self, item_id: i64, file_id: i64, window_start: Option<&str>) -> JfResult<i64> {
        let pins = self.t("playout_pins");
        let rows = self
            .inner
            .fetch_params(
                &format!(
                    "INSERT INTO {pins} (item_id, file_id, window_start, created_at) \
                     VALUES ($1::bigint, $2::bigint, $3::text, $4) RETURNING id"
                ),
                &[
                    Value::from(item_id),
                    Value::from(file_id),
                    window_start.map(|text| Value::String(text.to_string())).unwrap_or(Value::Null),
                    Value::String(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await?;
        Ok(rows.first().and_then(|row| row["id"].as_i64()).unwrap_or(0))
    }

    /// The last `limit` sync runs, newest first, for the health page.
    pub async fn recent_sync_runs(&self, limit: i64) -> JfResult<Vec<Value>> {
        let runs = self.t("sync_runs");
        self.inner
            .fetch_params(
                &format!(
                    "SELECT id, connection_id, kind, status, started_at, finished_at, \
                            items_added, items_updated, items_removed, errors, message \
                     FROM {runs} ORDER BY id DESC LIMIT $1::integer"
                ),
                &[Value::from(limit)],
            )
            .await
    }

    /// Record one sync run's outcome.
    pub async fn record_sync_run(
        &self,
        connection_id: i64,
        status: &str,
        added: u64,
        updated: u64,
        removed: u64,
        errors: u64,
        message: &str,
    ) -> JfResult<()> {
        let runs = self.t("sync_runs");
        self.inner
            .execute_params(
                &format!(
                    "INSERT INTO {runs} (connection_id, kind, status, started_at, finished_at, \
                     items_added, items_updated, items_removed, errors, message) \
                     VALUES ($1::integer, 'library', $2::text, $3, $4, $5::int, $6::int, $7::int, $8::int, $9::text)"
                ),
                &[
                    Value::from(connection_id),
                    Value::String(status.to_string()),
                    Value::String(chrono::Utc::now().to_rfc3339()),
                    Value::String(chrono::Utc::now().to_rfc3339()),
                    Value::from(added as i64),
                    Value::from(updated as i64),
                    Value::from(removed as i64),
                    Value::from(errors as i64),
                    Value::String(message.to_string()),
                ],
            )
            .await
            .map(|_| ())
    }
}

// ── raw-field helpers ──────────────────────────────────────────────────────

/// A PascalCase string field from a Jellyfin raw item (empty when missing).
fn jf(raw: &Value, field: &str) -> String {
    match raw.get(field) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// A numeric field from a Jellyfin raw item (or JSON null).
fn jf_num(raw: &Value, field: &str) -> Value {
    match raw.get(field) {
        Some(num) if num.is_number() => num.clone(),
        Some(Value::String(text)) => text.parse::<i64>().map(|value| Value::from(value)).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// A nullable integer as a bound text value.
fn value_or_null(number: Option<i32>) -> Value {
    number.map(|value| Value::from(value)).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_field_helpers_extract_jellyfin_json() {
        let raw = serde_json::json!({
            "Id": "abc",
            "Size": 1234,
            "Height": null,
            "IsRemote": true,
        });
        assert_eq!(jf(&raw, "Id"), "abc");
        assert_eq!(jf(&raw, "Missing"), "");
        assert_eq!(jf_num(&raw, "Size"), Value::from(1234));
        assert_eq!(jf_num(&raw, "Height"), Value::Null);
    }
}