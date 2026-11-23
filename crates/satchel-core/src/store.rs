//! SQLite-backed storage. One `Store` owns one connection; the app keeps it
//! behind a mutex on a dedicated thread. All SQL must be parameterized.
//!
//! Layout (see `MIGRATIONS`):
//! - `clips`: one small row per clip with everything a `ClipSummary` needs.
//!   Lists read only this table, so they never touch content blobs.
//! - `clip_data`: full text, HTML, image, thumbnail and file paths, keyed by
//!   clip id. Read one clip at a time.
//! - `clips_fts`: external-content FTS5 index (body, title, app name) over the
//!   `clips_fts_source` view, so text is not stored twice. Triggers keep it
//!   in sync, and FTS5 `secure-delete` really removes a deleted clip's terms
//!   (a contentless table would only tombstone them).
//! - `pinboards`, `clip_pins`, `settings`, `excluded_apps`.
//!
//! Dedup uses a SHA-256 of the payload behind a unique index.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::types::{Type, Value};
use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::classify;
use crate::error::{Error, Result};
use crate::model::{
    ClipContent, ClipId, ClipKind, ClipSummary, InsertOutcome, MAX_IMAGE_BYTES, MAX_TEXT_BYTES,
    Millis, NewClip, PREVIEW_CHARS, Pinboard, PinboardId, Query, Settings, SourceApp,
};

/// Schema version written to `PRAGMA user_version`: the number of applied
/// migrations.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Thumbnails above this size are dropped (the caller makes them at most
/// 320 px, which is far below this).
const MAX_THUMBNAIL_BYTES: usize = 1024 * 1024;
const MAX_TITLE_CHARS: usize = 256;
const MAX_PINBOARD_NAME_CHARS: usize = 100;
/// Search terms beyond this many are ignored.
const MAX_SEARCH_TOKENS: usize = 16;
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;
const DAY_MS: i64 = 86_400_000;

/// Ordered, append-only list of migrations. Never edit a released entry; add
/// a new one. Entry `i` moves the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[MIGRATION_1];

const MIGRATION_1: &str = r#"
CREATE TABLE clips (
    id            INTEGER PRIMARY KEY,
    kind          INTEGER NOT NULL,
    title         TEXT,
    preview       TEXT    NOT NULL,
    app_id        TEXT,
    app_name      TEXT,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER NOT NULL,
    byte_size     INTEGER NOT NULL,
    has_thumbnail INTEGER NOT NULL,
    content_hash  BLOB    NOT NULL
) STRICT;
CREATE UNIQUE INDEX clips_by_hash ON clips(content_hash);
CREATE INDEX clips_by_recent ON clips(last_used_at);
CREATE INDEX clips_by_kind_recent ON clips(kind, last_used_at);

CREATE TABLE clip_data (
    clip_id    INTEGER PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
    thumbnail  BLOB,
    file_paths BLOB,
    text       TEXT,
    html       TEXT,
    image      BLOB,
    -- What search sees when it is not `text` (file paths, one per line).
    search_body TEXT
) STRICT;

CREATE TABLE pinboards (
    id       INTEGER PRIMARY KEY,
    name     TEXT    NOT NULL,
    color    TEXT    NOT NULL,
    position INTEGER NOT NULL
) STRICT;

CREATE TABLE clip_pins (
    clip_id     INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    pinboard_id INTEGER NOT NULL REFERENCES pinboards(id) ON DELETE CASCADE,
    PRIMARY KEY (clip_id, pinboard_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX clip_pins_by_board ON clip_pins(pinboard_id, clip_id);

CREATE VIEW clips_fts_source AS
    SELECT c.id AS id,
           COALESCE(d.search_body, d.text, '') AS body,
           c.title AS title,
           c.app_name AS app
    FROM clips c JOIN clip_data d ON d.clip_id = c.id;

CREATE VIRTUAL TABLE clips_fts USING fts5(
    body, title, app,
    content = 'clips_fts_source',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
INSERT INTO clips_fts (clips_fts, rank) VALUES ('secure-delete', 1);

-- The index is maintained only here. Every 'delete' must pass exactly the
-- values that were indexed, so these expressions mirror the view.
CREATE TRIGGER clip_data_after_insert AFTER INSERT ON clip_data BEGIN
    INSERT INTO clips_fts (rowid, body, title, app)
    SELECT new.clip_id, COALESCE(new.search_body, new.text, ''), c.title, c.app_name
    FROM clips c WHERE c.id = new.clip_id;
END;

CREATE TRIGGER clips_before_delete BEFORE DELETE ON clips BEGIN
    INSERT INTO clips_fts (clips_fts, rowid, body, title, app)
    SELECT 'delete', old.id, COALESCE(d.search_body, d.text, ''), old.title, old.app_name
    FROM clip_data d WHERE d.clip_id = old.id;
END;

CREATE TRIGGER clips_after_title_update AFTER UPDATE OF title ON clips BEGIN
    INSERT INTO clips_fts (clips_fts, rowid, body, title, app)
    SELECT 'delete', old.id, COALESCE(d.search_body, d.text, ''), old.title, old.app_name
    FROM clip_data d WHERE d.clip_id = old.id;
    INSERT INTO clips_fts (rowid, body, title, app)
    SELECT new.id, COALESCE(d.search_body, d.text, ''), new.title, new.app_name
    FROM clip_data d WHERE d.clip_id = new.id;
END;

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value ANY
) STRICT, WITHOUT ROWID;

CREATE TABLE excluded_apps (
    position INTEGER PRIMARY KEY,
    app_id   TEXT NOT NULL
) STRICT;
"#;

const SUMMARY_COLUMNS: &str = "c.id, c.kind, c.title, c.preview, c.app_id, c.app_name, \
     c.created_at, c.last_used_at, c.byte_size, c.has_thumbnail, \
     (SELECT group_concat(p.pinboard_id) FROM clip_pins p WHERE p.clip_id = c.id)";

const NOT_PINNED: &str = "NOT EXISTS (SELECT 1 FROM clip_pins p WHERE p.clip_id = clips.id)";

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating and migrating if needed) the database at `path`. The
    /// file must end up readable and writable by the current user only.
    ///
    /// On Unix the parent directory is created if needed and set to 0700, and
    /// the file to 0600. A symlink at `path` is refused.
    pub fn open(path: &Path) -> Result<Self> {
        prepare_db_path(path)?;
        // No SQLITE_OPEN_URI (the path is a path, never a URI). Not
        // SQLITE_OPEN_NOFOLLOW either: it rejects a symlink anywhere in the
        // path (macOS `/var`, symlinked homes); `prepare_db_path` refuses a
        // symlink as the file itself instead.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        // WAL: readers never block the writer and commits are cheap. NORMAL
        // sync is durable across app crashes; only an OS crash can lose the
        // last commits, which is acceptable for clipboard history.
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Invalid(format!("could not enable WAL (got {mode})")));
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)?;
        Self::init(conn)
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        conn.pragma_update(None, "trusted_schema", false)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // Overwrite deleted content with zeros: history holds secrets.
        conn.pragma_update(None, "secure_delete", true)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        // 2 MiB page cache: the working set (summaries + FTS) is small.
        conn.pragma_update(None, "cache_size", -2048)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.set_prepared_statement_cache_capacity(48);
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// Store a clip. Content identical to an existing clip (by content hash)
    /// bumps that clip to the top instead of creating a duplicate. Enforces
    /// `MAX_TEXT_BYTES` / `MAX_IMAGE_BYTES`.
    ///
    /// Oversized HTML or thumbnails are dropped (the clip is kept without
    /// them); oversized text, images or file lists are rejected. The hash
    /// covers text, image and file paths, not HTML, kind or source app.
    pub fn insert(&mut self, clip: NewClip, now: Millis) -> Result<InsertOutcome> {
        let clip = PreparedClip::new(clip)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<ClipId> = tx
            .prepare_cached("SELECT id FROM clips WHERE content_hash = ?1")?
            .query_row([&clip.hash[..]], |r| r.get(0))
            .optional()?;
        let outcome_is_bump = existing.is_some();
        let id = match existing {
            Some(id) => {
                tx.prepare_cached("UPDATE clips SET last_used_at = ?2 WHERE id = ?1")?
                    .execute(params![id, now])?;
                id
            }
            None => {
                let (app_id, app_name) = match &clip.source_app {
                    Some(a) => (Some(a.id.as_str()), Some(a.name.as_str())),
                    None => (None, None),
                };
                tx.prepare_cached(
                    "INSERT INTO clips (kind, title, preview, app_id, app_name, created_at, \
                     last_used_at, byte_size, has_thumbnail, content_hash) \
                     VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![
                    kind_code(clip.kind),
                    clip.preview,
                    app_id,
                    app_name,
                    now,
                    clip.byte_size as i64,
                    clip.thumbnail.is_some(),
                    &clip.hash[..],
                ])?;
                let id = tx.last_insert_rowid();
                // Indexed for search by the `clip_data_after_insert` trigger.
                tx.prepare_cached(
                    "INSERT INTO clip_data \
                     (clip_id, thumbnail, file_paths, text, html, image, search_body) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?
                .execute(params![
                    id,
                    clip.thumbnail,
                    clip.file_paths,
                    clip.text,
                    clip.html,
                    clip.image,
                    clip.search_body,
                ])?;
                id
            }
        };
        let summary = summary_of(&tx, id)?.ok_or(Error::NotFound)?;
        tx.commit()?;
        Ok(if outcome_is_bump {
            InsertOutcome::Bumped(summary)
        } else {
            InsertOutcome::Inserted(summary)
        })
    }

    /// Newest-first page of summaries matching `query`.
    ///
    /// Search splits the input into words (letters and digits), matches each
    /// as a prefix (so results update while typing), and requires all of them.
    /// Input without any word does not filter.
    pub fn list(&self, query: &Query) -> Result<Vec<ClipSummary>> {
        let fts = query.text.as_deref().and_then(fts_query);
        let limit = match query.limit {
            0 => DEFAULT_LIMIT,
            n => n.min(MAX_LIMIT),
        };
        let kind = query.kind.map(kind_code);
        let filters = ListFilters {
            text: fts.is_some(),
            kind: kind.is_some(),
            pinboard: query.pinboard_id.is_some(),
            before: query.before.is_some(),
        };
        let sql = list_sql(filters);
        let (before_ts, before_id) = query.before.unwrap_or((0, 0));
        let mut args: Vec<(&str, &dyn ToSql)> = vec![(":limit", &limit)];
        if let Some(q) = &fts {
            args.push((":q", q));
        }
        if let Some(k) = &kind {
            args.push((":kind", k));
        }
        if let Some(b) = &query.pinboard_id {
            args.push((":board", b));
        }
        if filters.before {
            args.push((":ts", &before_ts));
            args.push((":id", &before_id));
        }
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(args.as_slice(), summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn summary(&self, id: ClipId) -> Result<Option<ClipSummary>> {
        summary_of(&self.conn, id)
    }

    /// Full payload of one clip.
    pub fn content(&self, id: ClipId) -> Result<Option<ClipContent>> {
        let row = self
            .conn
            .prepare_cached(
                "SELECT c.kind, d.text, d.html, d.image, d.file_paths \
                 FROM clips c JOIN clip_data d ON d.clip_id = c.id WHERE c.id = ?1",
            )?
            .query_row([id], |r| {
                Ok(ClipContent {
                    id,
                    kind: kind_from_row(r, 0)?,
                    text: r.get(1)?,
                    html: r.get(2)?,
                    image_png: r.get(3)?,
                    file_paths: decode_paths(r.get::<_, Option<Vec<u8>>>(4)?.as_deref()),
                })
            })
            .optional()?;
        Ok(row)
    }

    /// PNG thumbnail bytes of an image clip.
    pub fn thumbnail(&self, id: ClipId) -> Result<Option<Vec<u8>>> {
        let thumb: Option<Option<Vec<u8>>> = self
            .conn
            .prepare_cached("SELECT thumbnail FROM clip_data WHERE clip_id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        Ok(thumb.flatten())
    }

    /// Mark a clip as just used: moves it to the top of the history.
    pub fn touch(&mut self, id: ClipId, now: Millis) -> Result<()> {
        let n = self
            .conn
            .prepare_cached("UPDATE clips SET last_used_at = ?2 WHERE id = ?1")?
            .execute(params![id, now])?;
        if n == 0 { Err(Error::NotFound) } else { Ok(()) }
    }

    /// Blank titles clear the title. Titles longer than 256 characters are
    /// rejected.
    pub fn set_title(&mut self, id: ClipId, title: Option<&str>) -> Result<()> {
        let title = title.map(str::trim).filter(|t| !t.is_empty());
        if let Some(t) = title
            && t.chars().count() > MAX_TITLE_CHARS
        {
            return Err(Error::Invalid(format!(
                "title longer than {MAX_TITLE_CHARS} characters"
            )));
        }
        // Re-indexed by the `clips_after_title_update` trigger.
        let n = self
            .conn
            .prepare_cached("UPDATE clips SET title = ?2 WHERE id = ?1")?
            .execute(params![id, title])?;
        if n == 0 { Err(Error::NotFound) } else { Ok(()) }
    }

    /// Returns true when a row was deleted.
    ///
    /// The clip's pages are zeroed (`secure_delete`), its index entries are
    /// removed (FTS5 `secure-delete`) and the WAL is checkpointed and
    /// truncated, so its content does not stay in the database files.
    pub fn delete(&mut self, id: ClipId) -> Result<bool> {
        let n = self
            .conn
            .prepare_cached("DELETE FROM clips WHERE id = ?1")?
            .execute([id])?;
        if n > 0 {
            self.checkpoint()?;
        }
        Ok(n > 0)
    }

    /// Delete every clip that is not on a pinboard. Returns the count.
    ///
    /// Like `delete`, removed content does not linger in the files.
    pub fn clear_history(&mut self) -> Result<u64> {
        let n = self
            .conn
            .prepare_cached(&format!("DELETE FROM clips WHERE {NOT_PINNED}"))?
            .execute([])?;
        if n > 0 {
            self.checkpoint()?;
        }
        Ok(n as u64)
    }

    /// Apply `max_items` and `retention_days` to unpinned clips. Returns the
    /// number of clips removed.
    ///
    /// `max_items` counts unpinned clips only; 0 means no count limit.
    /// Age is measured from `last_used_at`.
    pub fn prune(&mut self, settings: &Settings, now: Millis) -> Result<u64> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed = 0usize;
        if let Some(days) = settings.retention_days {
            let cutoff = now.saturating_sub(i64::from(days).saturating_mul(DAY_MS));
            removed += tx
                .prepare_cached(&format!(
                    "DELETE FROM clips WHERE last_used_at < ?1 AND {NOT_PINNED}"
                ))?
                .execute([cutoff])?;
        }
        if settings.max_items > 0 {
            removed += tx
                .prepare_cached(&format!(
                    "DELETE FROM clips WHERE id IN (SELECT id FROM clips WHERE {NOT_PINNED} \
                     ORDER BY last_used_at DESC, id DESC LIMIT -1 OFFSET ?1)"
                ))?
                .execute([i64::from(settings.max_items)])?;
        }
        tx.commit()?;
        if removed > 0 {
            self.checkpoint()?;
        }
        Ok(removed as u64)
    }

    pub fn pinboards(&self) -> Result<Vec<Pinboard>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, color, position FROM pinboards ORDER BY position, id",
        )?;
        let rows = stmt.query_map([], pinboard_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `name` is trimmed and must be 1..=100 characters; `color` is any color
    /// `classify::parse_color` accepts without alpha, stored as `#RRGGBB`.
    pub fn create_pinboard(&mut self, name: &str, color: &str) -> Result<Pinboard> {
        let name = valid_pinboard_name(name)?;
        let color = classify::parse_color(color)
            .filter(|c| c.len() == 7)
            .ok_or_else(|| Error::Invalid("pinboard color must be #RRGGBB".into()))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let position: i64 = tx
            .prepare_cached("SELECT COALESCE(MAX(position) + 1, 0) FROM pinboards")?
            .query_row([], |r| r.get(0))?;
        tx.prepare_cached("INSERT INTO pinboards (name, color, position) VALUES (?1, ?2, ?3)")?
            .execute(params![name, color, position])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Pinboard {
            id,
            name: name.to_string(),
            color,
            position,
        })
    }

    pub fn rename_pinboard(&mut self, id: PinboardId, name: &str) -> Result<()> {
        let name = valid_pinboard_name(name)?;
        let n = self
            .conn
            .prepare_cached("UPDATE pinboards SET name = ?2 WHERE id = ?1")?
            .execute(params![id, name])?;
        if n == 0 { Err(Error::NotFound) } else { Ok(()) }
    }

    /// Deletes the pinboard; its clips stay in the history.
    pub fn delete_pinboard(&mut self, id: PinboardId) -> Result<()> {
        let n = self
            .conn
            .prepare_cached("DELETE FROM pinboards WHERE id = ?1")?
            .execute([id])?;
        if n == 0 { Err(Error::NotFound) } else { Ok(()) }
    }

    /// Idempotent. `NotFound` when the clip or the pinboard does not exist.
    pub fn pin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()> {
        let n = self
            .conn
            .prepare_cached(
                "INSERT OR IGNORE INTO clip_pins (clip_id, pinboard_id) \
                 SELECT c.id, b.id FROM clips c, pinboards b WHERE c.id = ?1 AND b.id = ?2",
            )?
            .execute(params![clip, pinboard])?;
        if n > 0 {
            return Ok(());
        }
        let exists: bool = self
            .conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM clips WHERE id = ?1) \
                 AND EXISTS (SELECT 1 FROM pinboards WHERE id = ?2)",
            )?
            .query_row(params![clip, pinboard], |r| r.get(0))?;
        if exists { Ok(()) } else { Err(Error::NotFound) }
    }

    /// Idempotent.
    pub fn unpin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()> {
        self.conn
            .prepare_cached("DELETE FROM clip_pins WHERE clip_id = ?1 AND pinboard_id = ?2")?
            .execute(params![clip, pinboard])?;
        Ok(())
    }

    /// Stored settings, or `Settings::default()` when none were saved.
    /// Fields missing from storage keep their default.
    pub fn settings(&self) -> Result<Settings> {
        let mut s = Settings::default();
        let mut has_exclusions = false;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, value FROM settings")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let value: Value = row.get(1)?;
            match (key.as_str(), value) {
                ("max_items", Value::Integer(v)) => {
                    s.max_items = u32::try_from(v).unwrap_or(s.max_items);
                }
                ("retention_days", Value::Null) => s.retention_days = None,
                ("retention_days", Value::Integer(v)) => {
                    s.retention_days = u32::try_from(v).ok().or(s.retention_days);
                }
                ("capture_paused", Value::Integer(v)) => s.capture_paused = v != 0,
                ("capture_images", Value::Integer(v)) => s.capture_images = v != 0,
                ("paste_on_select", Value::Integer(v)) => s.paste_on_select = v != 0,
                ("shelf_shortcut", Value::Text(v)) => s.shelf_shortcut = v,
                ("excluded_app_ids", Value::Integer(_)) => has_exclusions = true,
                _ => {} // unknown or malformed: keep the default
            }
        }
        if has_exclusions {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT app_id FROM excluded_apps ORDER BY position")?;
            let ids = stmt.query_map([], |r| r.get(0))?;
            s.excluded_app_ids = ids.collect::<rusqlite::Result<_>>()?;
        }
        Ok(s)
    }

    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        if settings.shelf_shortcut.len() > 256 {
            return Err(Error::Invalid("shortcut too long".into()));
        }
        if settings.excluded_app_ids.len() > 10_000
            || settings.excluded_app_ids.iter().any(|id| id.len() > 1024)
        {
            return Err(Error::Invalid("excluded app list too large".into()));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut put = tx.prepare_cached(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?;
            put.execute(params!["max_items", settings.max_items])?;
            put.execute(params!["retention_days", settings.retention_days])?;
            put.execute(params!["capture_paused", settings.capture_paused])?;
            put.execute(params!["capture_images", settings.capture_images])?;
            put.execute(params!["paste_on_select", settings.paste_on_select])?;
            put.execute(params!["shelf_shortcut", settings.shelf_shortcut])?;
            put.execute(params![
                "excluded_app_ids",
                settings.excluded_app_ids.len() as i64
            ])?;
            tx.prepare_cached("DELETE FROM excluded_apps")?
                .execute([])?;
            let mut add =
                tx.prepare_cached("INSERT INTO excluded_apps (position, app_id) VALUES (?1, ?2)")?;
            for (i, id) in settings.excluded_app_ids.iter().enumerate() {
                add.execute(params![i as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Fold the WAL into the main file and truncate it. A no-op in memory.
    fn checkpoint(&self) -> Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}

// ------------------------------------------------------------ opening

#[cfg(unix)]
fn prepare_db_path(path: &Path) -> Result<()> {
    use std::fs::{DirBuilder, OpenOptions, Permissions};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        std::fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(Error::Invalid("database path is a symlink".into()));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(Error::Invalid("database path is not a regular file".into()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)?;
        }
        Err(e) => return Err(e.into()),
    }
    std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_db_path(path: &Path) -> Result<()> {
    // Per-user app data directories on Windows are already private to the
    // user through inherited ACLs.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if !(0..=SCHEMA_VERSION).contains(&version) {
        return Err(Error::Invalid(format!(
            "database schema version {version} is not supported (this build knows up to {SCHEMA_VERSION})"
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
    }
    tx.commit()?;
    Ok(())
}

// --------------------------------------------------------- insert prep

/// A validated `NewClip` with derived columns, ready to write.
struct PreparedClip {
    kind: ClipKind,
    text: Option<String>,
    html: Option<String>,
    image: Option<Vec<u8>>,
    thumbnail: Option<Vec<u8>>,
    /// Paths joined with NUL, which no OS allows inside a path.
    file_paths: Option<Vec<u8>>,
    /// Search text when it is not `text`: file paths, one per line.
    search_body: Option<String>,
    source_app: Option<SourceApp>,
    preview: String,
    byte_size: u64,
    hash: [u8; 32],
}

fn too_large(size: usize, limit: u64) -> Error {
    Error::TooLarge {
        size: size as u64,
        limit,
    }
}

impl PreparedClip {
    fn new(clip: NewClip) -> Result<Self> {
        let NewClip {
            kind,
            text,
            mut html,
            image_png,
            mut thumbnail_png,
            file_paths,
            source_app,
        } = clip;

        if let Some(t) = &text
            && t.len() as u64 > MAX_TEXT_BYTES
        {
            return Err(too_large(t.len(), MAX_TEXT_BYTES));
        }
        if let Some(i) = &image_png
            && i.len() as u64 > MAX_IMAGE_BYTES
        {
            return Err(too_large(i.len(), MAX_IMAGE_BYTES));
        }
        if html
            .as_ref()
            .is_some_and(|h| h.len() as u64 > MAX_TEXT_BYTES)
        {
            html = None;
        }
        if thumbnail_png
            .as_ref()
            .is_some_and(|t| t.len() > MAX_THUMBNAIL_BYTES || t.is_empty())
        {
            thumbnail_png = None;
        }
        let paths_len: usize = file_paths.iter().map(|p| p.len() + 1).sum();
        if paths_len as u64 > MAX_TEXT_BYTES {
            return Err(too_large(paths_len, MAX_TEXT_BYTES));
        }
        if file_paths.iter().any(|p| p.is_empty() || p.contains('\0')) {
            return Err(Error::Invalid(
                "file paths must be non-empty and contain no NUL".into(),
            ));
        }

        match kind {
            ClipKind::Text | ClipKind::Link | ClipKind::Code | ClipKind::Color => {
                if text.as_deref().is_none_or(str::is_empty) {
                    return Err(Error::Invalid("text clip without text".into()));
                }
            }
            ClipKind::Image => {
                if image_png.as_deref().is_none_or(<[u8]>::is_empty) {
                    return Err(Error::Invalid("image clip without image data".into()));
                }
            }
            ClipKind::File => {
                if file_paths.is_empty() {
                    return Err(Error::Invalid("file clip without paths".into()));
                }
            }
        }

        let preview = match kind {
            ClipKind::Image => String::new(),
            ClipKind::File => file_preview(&file_paths),
            ClipKind::Color => {
                let t = text.as_deref().unwrap_or_default();
                classify::parse_color(t).unwrap_or_else(|| classify::preview(t, PREVIEW_CHARS))
            }
            _ => classify::preview(text.as_deref().unwrap_or_default(), PREVIEW_CHARS),
        };

        let hash = content_hash(text.as_deref(), image_png.as_deref(), &file_paths);
        let byte_size = (text.as_ref().map_or(0, String::len)
            + image_png.as_ref().map_or(0, Vec::len)
            + file_paths.iter().map(String::len).sum::<usize>()) as u64;
        let search_body = (kind == ClipKind::File).then(|| file_paths.join("\n"));
        let file_paths = (!file_paths.is_empty()).then(|| file_paths.join("\0").into_bytes());

        Ok(Self {
            kind,
            text,
            html,
            image: image_png,
            thumbnail: thumbnail_png,
            file_paths,
            search_body,
            source_app,
            preview,
            byte_size,
            hash,
        })
    }
}

/// SHA-256 over a tagged, length-prefixed encoding of the payload, so that
/// different payload types with the same bytes never collide.
fn content_hash(text: Option<&str>, image: Option<&[u8]>, paths: &[String]) -> [u8; 32] {
    fn field(h: &mut Sha256, tag: u8, bytes: Option<&[u8]>) {
        h.update([tag]);
        match bytes {
            Some(b) => {
                h.update([1]);
                h.update((b.len() as u64).to_le_bytes());
                h.update(b);
            }
            None => h.update([0]),
        }
    }
    let mut h = Sha256::new();
    h.update(b"satchel-clip-v1");
    field(&mut h, b't', text.map(str::as_bytes));
    field(&mut h, b'i', image);
    h.update(b"f");
    h.update((paths.len() as u64).to_le_bytes());
    for p in paths {
        field(&mut h, b'p', Some(p.as_bytes()));
    }
    h.finalize().into()
}

fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let trimmed = if trimmed.is_empty() { path } else { trimmed };
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

fn file_preview(paths: &[String]) -> String {
    let mut names = String::new();
    for p in paths {
        if names.len() > PREVIEW_CHARS * 4 {
            break;
        }
        if !names.is_empty() {
            names.push('\n');
        }
        names.push_str(file_name(p));
    }
    classify::preview(&names, PREVIEW_CHARS)
}

fn decode_paths(blob: Option<&[u8]>) -> Vec<String> {
    match blob {
        None | Some([]) => Vec::new(),
        Some(b) => b
            .split(|&c| c == 0)
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect(),
    }
}

fn valid_pinboard_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_PINBOARD_NAME_CHARS {
        return Err(Error::Invalid(format!(
            "pinboard name must be 1 to {MAX_PINBOARD_NAME_CHARS} characters"
        )));
    }
    Ok(name)
}

// ------------------------------------------------------------- search

/// Turns user input into a safe FTS5 query: every run of letters/digits
/// becomes a quoted prefix term (`"word"*`), terms are ANDed. Quoting makes
/// FTS operators (`AND`, `NEAR`, `col:`, `^`, `-`) plain words; splitting on
/// everything else means no quote can reach the query. Returns `None` when
/// the input has no words.
pub(crate) fn fts_query(input: &str) -> Option<String> {
    let mut out = String::new();
    for token in input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(MAX_SEARCH_TOKENS)
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('"');
        out.push_str(token);
        out.push_str("\"*");
    }
    (!out.is_empty()).then_some(out)
}

#[derive(Clone, Copy, Debug)]
struct ListFilters {
    text: bool,
    kind: bool,
    pinboard: bool,
    before: bool,
}

/// One SQL text per filter combination (16 at most), so each gets its own
/// cached statement and query plan. Only static fragments are concatenated;
/// every value is a bound parameter.
fn list_sql(f: ListFilters) -> String {
    let mut sql = format!("SELECT {SUMMARY_COLUMNS} FROM clips c WHERE 1");
    if f.text {
        sql.push_str(" AND c.id IN (SELECT rowid FROM clips_fts WHERE clips_fts MATCH :q)");
    }
    if f.kind {
        sql.push_str(" AND c.kind = :kind");
    }
    if f.pinboard {
        sql.push_str(" AND c.id IN (SELECT clip_id FROM clip_pins WHERE pinboard_id = :board)");
    }
    if f.before {
        sql.push_str(" AND c.last_used_at <= :ts AND (c.last_used_at < :ts OR c.id < :id)");
    }
    let _ = write!(sql, " ORDER BY c.last_used_at DESC, c.id DESC LIMIT :limit");
    sql
}

// ----------------------------------------------------------- row maps

fn summary_of(conn: &Connection, id: ClipId) -> Result<Option<ClipSummary>> {
    let sql = format!("SELECT {SUMMARY_COLUMNS} FROM clips c WHERE c.id = ?1");
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([id], summary_from_row)
        .optional()?)
}

fn summary_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClipSummary> {
    let app_id: Option<String> = r.get(4)?;
    let app_name: Option<String> = r.get(5)?;
    let pins: Option<String> = r.get(10)?;
    let mut pinboard_ids: Vec<PinboardId> = pins
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    pinboard_ids.sort_unstable();
    Ok(ClipSummary {
        id: r.get(0)?,
        kind: kind_from_row(r, 1)?,
        title: r.get(2)?,
        preview: r.get(3)?,
        source_app: app_id.map(|id| SourceApp {
            id,
            name: app_name.unwrap_or_default(),
        }),
        created_at: r.get(6)?,
        last_used_at: r.get(7)?,
        byte_size: r.get::<_, i64>(8)?.max(0) as u64,
        has_thumbnail: r.get(9)?,
        pinboard_ids,
    })
}

fn pinboard_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pinboard> {
    Ok(Pinboard {
        id: r.get(0)?,
        name: r.get(1)?,
        color: r.get(2)?,
        position: r.get(3)?,
    })
}

/// Stable on-disk codes. Never renumber; only append.
fn kind_code(kind: ClipKind) -> i64 {
    match kind {
        ClipKind::Text => 0,
        ClipKind::Link => 1,
        ClipKind::Code => 2,
        ClipKind::Color => 3,
        ClipKind::Image => 4,
        ClipKind::File => 5,
    }
}

fn kind_from_code(code: i64) -> Option<ClipKind> {
    Some(match code {
        0 => ClipKind::Text,
        1 => ClipKind::Link,
        2 => ClipKind::Code,
        3 => ClipKind::Color,
        4 => ClipKind::Image,
        5 => ClipKind::File,
        _ => return None,
    })
}

fn kind_from_row(r: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<ClipKind> {
    let code: i64 = r.get(idx)?;
    kind_from_code(code).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(idx, Type::Integer, "unknown clip kind".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> NewClip {
        NewClip {
            kind: ClipKind::Text,
            text: Some(t.into()),
            html: None,
            image_png: None,
            thumbnail_png: None,
            file_paths: vec![],
            source_app: Some(SourceApp {
                id: "app.id".into(),
                name: "Some App".into(),
            }),
        }
    }

    #[test]
    fn store_can_move_to_a_dedicated_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<Store>();
    }

    #[test]
    fn bundled_sqlite_is_current_and_has_fts5() {
        let s = Store::open_in_memory().unwrap();
        let version: String = s
            .conn
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .unwrap();
        let parts: Vec<u32> = version.split('.').map(|p| p.parse().unwrap()).collect();
        assert!(
            parts[0] == 3 && parts[1] >= 45,
            "SQLite {version} is too old"
        );
        let mut stmt = s.conn.prepare("PRAGMA compile_options").unwrap();
        let opts: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(opts.iter().any(|o| o == "ENABLE_FTS5"), "{opts:?}");
    }

    #[test]
    fn fts_query_quotes_every_word_as_a_prefix() {
        assert_eq!(fts_query("hello"), Some(r#""hello"*"#.into()));
        assert_eq!(
            fts_query("  Hello, wor "),
            Some(r#""Hello"* "wor"*"#.into())
        );
        assert_eq!(
            fts_query(r#"a"b OR c:d NEAR(e) -f ^g *"#),
            Some(r#""a"* "b"* "OR"* "c"* "d"* "NEAR"* "e"* "f"* "g"*"#.into())
        );
        assert_eq!(fts_query("café 日本"), Some(r#""café"* "日本"*"#.into()));
        assert_eq!(fts_query(""), None);
        assert_eq!(fts_query(" !?\"*() "), None);
        let many = "w ".repeat(100);
        assert_eq!(
            fts_query(&many).unwrap().matches('*').count(),
            MAX_SEARCH_TOKENS
        );
    }

    fn plan(s: &Store, f: ListFilters) -> String {
        let sql = format!("EXPLAIN QUERY PLAN {}", list_sql(f));
        let mut stmt = s.conn.prepare(&sql).unwrap();
        let (q, kind, board, ts, id, limit) = ("\"x\"*".to_string(), 0i64, 1i64, 5i64, 5i64, 50i64);
        let mut args: Vec<(&str, &dyn ToSql)> = vec![(":limit", &limit)];
        if f.text {
            args.push((":q", &q));
        }
        if f.kind {
            args.push((":kind", &kind));
        }
        if f.pinboard {
            args.push((":board", &board));
        }
        if f.before {
            args.push((":ts", &ts));
            args.push((":id", &id));
        }
        let rows = stmt
            .query_map(args.as_slice(), |r| r.get::<_, String>(3))
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n")
    }

    #[test]
    fn list_never_reads_content_blobs() {
        let s = Store::open_in_memory().unwrap();
        for bits in 0u8..16 {
            let f = ListFilters {
                text: bits & 1 != 0,
                kind: bits & 2 != 0,
                pinboard: bits & 4 != 0,
                before: bits & 8 != 0,
            };
            let p = plan(&s, f);
            assert!(!p.contains("clip_data"), "{f:?} reads clip_data:\n{p}");
        }
    }

    #[test]
    fn first_page_walks_the_recency_index_without_sorting() {
        let s = Store::open_in_memory().unwrap();
        for before in [false, true] {
            let f = ListFilters {
                text: false,
                kind: false,
                pinboard: false,
                before,
            };
            let p = plan(&s, f);
            assert!(p.contains("clips_by_recent"), "{p}");
            assert!(!p.contains("TEMP B-TREE"), "{p}");
        }
        let f = ListFilters {
            text: false,
            kind: true,
            pinboard: false,
            before: true,
        };
        let p = plan(&s, f);
        assert!(
            p.contains("clips_by_kind_recent") && !p.contains("TEMP B-TREE"),
            "{p}"
        );
    }

    fn assert_index_consistent(s: &Store) {
        // rank = 1 also compares the index with the content view.
        s.conn
            .execute(
                "INSERT INTO clips_fts (clips_fts, rank) VALUES ('integrity-check', 1)",
                [],
            )
            .expect("FTS index out of sync with content");
    }

    #[test]
    fn integrity_check_detects_a_mismatched_delete() {
        let mut s = Store::open_in_memory().unwrap();
        s.insert(text("alpha beta"), 1).unwrap();
        assert_index_consistent(&s);
        s.conn
            .execute(
                "INSERT INTO clips_fts (clips_fts, rowid, body, title, app) \
                 VALUES ('delete', 1, 'gamma', NULL, 'Some App')",
                [],
            )
            .unwrap();
        let check = s.conn.execute(
            "INSERT INTO clips_fts (clips_fts, rank) VALUES ('integrity-check', 1)",
            [],
        );
        assert!(check.is_err());
    }

    #[test]
    fn search_index_stays_in_sync_through_every_mutation() {
        let mut s = Store::open_in_memory().unwrap();
        let board = s.create_pinboard("B", "#000000").unwrap();
        let mut ids = Vec::new();
        for i in 0..30 {
            let out = s.insert(text(&format!("entry {i} with words")), i).unwrap();
            let (InsertOutcome::Inserted(c) | InsertOutcome::Bumped(c)) = out;
            ids.push(c.id);
        }
        let files = NewClip {
            kind: ClipKind::File,
            text: Some("file:///ignored".into()),
            file_paths: vec!["/a/b.txt".into(), "/c/d e.pdf".into()],
            ..text("x")
        };
        s.insert(files, 100).unwrap();
        s.insert(
            NewClip {
                kind: ClipKind::Image,
                text: None,
                image_png: Some(vec![1]),
                ..text("x")
            },
            101,
        )
        .unwrap();
        s.insert(text("entry 3 with words"), 200).unwrap(); // bump
        assert_index_consistent(&s);
        s.set_title(ids[0], Some("first title")).unwrap();
        s.set_title(ids[0], Some("second title")).unwrap();
        s.set_title(ids[1], Some("t")).unwrap();
        s.set_title(ids[1], None).unwrap();
        assert_index_consistent(&s);
        s.pin(ids[0], board.id).unwrap();
        assert!(s.delete(ids[2]).unwrap());
        assert_index_consistent(&s);
        s.prune(
            &Settings {
                max_items: 10,
                retention_days: None,
                ..Settings::default()
            },
            300,
        )
        .unwrap();
        assert_index_consistent(&s);
        s.clear_history().unwrap();
        assert_index_consistent(&s);
        let left: i64 = s
            .conn
            .query_row("SELECT count(*) FROM clips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }
}
