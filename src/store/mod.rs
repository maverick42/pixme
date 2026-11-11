//! Session persistence: the viewer's tabs mirror into a small SQLite
//! database so a normal quit preserves every capture — document,
//! annotations and the chronological undo/redo stacks — and the next
//! viewer open restores them silently.
//!
//! Writes are eager, not quit-time: a dedicated [`writer::Writer`]
//! thread owns the `Store` for the whole run and drains an ordered job
//! queue — every document lands on disk shortly after the change that
//! created it, so a crash or kill only loses what happened in the last
//! few hundred milliseconds. PNG encoding and SQLite I/O never run on
//! the UI thread; quitting flushes the queue behind a bounded wait.
//!
//! One row per tab; per row, three postcard blobs (`annotations`,
//! `anno_undo`, `anno_redo`) hold the object model and its history —
//! bitmaps inside them ride as PNG bytes via [`ser::rgba_png`]. The
//! pixel-level ops (Resize) are NOT persisted: their payloads are
//! whole pre-op images, far too heavy for a small database.

pub mod ser;
pub mod writer;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::editor::Document;
use crate::editor::history::AnnoEdit;
use crate::editor::meta::CaptureMeta;
use crate::ui::viewer::Tab;

/// Current layout — a file carrying another version is wiped clean
/// rather than migrated (the session is a convenience, not an archive).
/// v15 is the one exception: it only adds `workspace_id`, so a v14 file
/// is ALTERed in place and keeps every capture (they all land in `main`).
const SCHEMA_VERSION: i64 = 15;

/// Days a `.bak` recovery copy survives a rebuild — the captures it
/// contains must not linger forever.
const BAK_KEEP_DAYS: u64 = 7;

const DDL_META: &str = "
    CREATE TABLE IF NOT EXISTS meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

const DDL_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS session (
        id         INTEGER PRIMARY KEY CHECK (id = 0),
        active     INTEGER NOT NULL DEFAULT 0,
        next_title INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS documents (
        doc_id        TEXT    PRIMARY KEY,
        ord           INTEGER NOT NULL,
        title         TEXT    NOT NULL,
        saved         INTEGER NOT NULL,
        captured_at   TEXT    NOT NULL,
        captured_epoch INTEGER NOT NULL DEFAULT 0,
        saved_at      TEXT,
        source_path   TEXT,
        password_hash TEXT,
        workspace_id  TEXT    NOT NULL DEFAULT 'main',
        base_png      BLOB    NOT NULL,
        scale_factor  REAL    NOT NULL,
        monitor_json  TEXT    NOT NULL,
        next_id       INTEGER NOT NULL,
        annotations   BLOB    NOT NULL,
        anno_undo     BLOB    NOT NULL,
        anno_redo     BLOB    NOT NULL
    );
";

/// One tab as it comes back from the database.
pub struct StoredTab {
    /// Tab label (already unique — it was when written).
    pub title: String,
    /// Whether the document matched disk when the session was written.
    pub saved: bool,
    /// Capture + metadata + annotations + id counter.
    pub doc: Document,
    /// Chronological undo stack, restored as-is.
    pub anno_undo: VecDeque<AnnoEdit>,
    /// Redo side of the stack.
    pub anno_redo: VecDeque<AnnoEdit>,
}

/// The persisted session, ready to hand to `Viewer::restore_session`.
#[derive(Default)]
pub struct StoredSession {
    /// Tabs in display order.
    pub tabs: Vec<StoredTab>,
    /// Which tab was active.
    pub active: usize,
    /// The auto-title counter (`Capture N` keeps counting up).
    pub next_title: usize,
}

/// One document as the writer must persist it — a UI-side snapshot
/// cloned cheaply (the base image rides an `Arc`, postcards are
/// encoded on the worker). Produced by `Tab::to_doc_write`.
pub struct DocWrite {
    /// `doc.meta.id` — the row's primary key.
    pub id: String,
    /// Display position when the snapshot was taken.
    pub ord: i64,
    /// Tab label.
    pub title: String,
    /// Clean flag (matches disk).
    pub saved: bool,
    /// Capture instant (display + retention cutoff).
    pub captured_at: chrono::DateTime<chrono::Local>,
    /// Last successful file save, if any.
    pub saved_at: Option<chrono::DateTime<chrono::Local>>,
    /// File this document was loaded from / saved to.
    pub source_path: Option<PathBuf>,
    /// Protection hash (`None` = unprotected).
    pub password_hash: Option<String>,
    /// The capture's workspace — a real SQL column (kept OUT of the
    /// postcard blobs: adding a field there would break decoding of
    /// pre-v15 rows).
    pub workspace_id: String,
    /// `Some` ⇒ the base pixels changed: re-encode and replace
    /// `base_png`. `None` ⇒ keep the stored blob (annotation/meta
    /// edit — the expensive column is untouched).
    pub base: Option<std::sync::Arc<image::RgbaImage>>,
    /// HiDPI factor of the captured frame.
    pub scale_factor: f32,
    /// Monitor record the capture came from.
    pub monitor: crate::capture::MonitorInfo,
    /// Annotation id counter — restores without re-issuing ids.
    pub next_id: u64,
    /// `doc.revision` of the snapshot — informational (telemetry).
    pub revision: u64,
    /// The object model.
    pub annotations: Vec<crate::editor::document::Annotation>,
    /// Chronological undo stack.
    pub anno_undo: VecDeque<AnnoEdit>,
    /// Redo side of the stack.
    pub anno_redo: VecDeque<AnnoEdit>,
    /// When the job was queued — the enqueue→committed span is the
    /// crash-loss window, logged per write.
    pub queued_at: std::time::Instant,
}

/// The meta columns of one document row — everything EXCEPT the heavy
/// blobs (`base_png`, annotations, history). Sent for reorders,
/// renames, save-state flips and the quit-time refresh.
pub struct MetaRow {
    /// `doc.meta.id` — the row's primary key.
    pub id: String,
    /// Display position.
    pub ord: i64,
    /// Tab label.
    pub title: String,
    /// Clean flag.
    pub saved: bool,
    /// Last successful file save, if any.
    pub saved_at: Option<chrono::DateTime<chrono::Local>>,
    /// File this document was loaded from / saved to.
    pub source_path: Option<PathBuf>,
    /// Protection hash.
    pub password_hash: Option<String>,
}

/// The session database — a `None` connection means the file could not
/// be opened at all and persistence is silently off (a resident app
/// must never fail to launch over a store problem).
pub struct Store {
    conn: Option<Connection>,
}

impl Store {
    /// Opens the database at the default per-user location
    /// (`data_dir()/session.db`), creating it and its parent dir.
    pub fn open() -> Self {
        Self::open_at(db_path())
    }

    /// An in-memory store for unit tests — real SQL, no file on disk.
    #[cfg(test)]
    pub fn open_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("sqlite memory db");
        migrate(&conn).expect("migrate memory db");
        Self { conn: Some(conn) }
    }

    /// Opens (and migrates) the database at `path`. A file that fails to
    /// open or migrate once is moved aside to `<name>.bak` and rebuilt;
    /// a second failure leaves persistence disabled, logged.
    pub fn open_at(path: PathBuf) -> Self {
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("cannot create {}: {err}", crate::paths::file_name(dir));
        }
        match try_open(&path) {
            Ok(conn) => {
                prune_old_backup(&path);
                Self { conn: Some(conn) }
            }
            Err(first) => {
                let bak = bak_path(&path);
                if let Err(err) = std::fs::rename(&path, &bak) {
                    tracing::warn!(
                        "cannot park bad session db as {}: {err}",
                        crate::paths::file_name(&bak)
                    );
                }
                match try_open(&path) {
                    Ok(conn) => {
                        tracing::warn!(
                            "session db rebuilt ({first:#}); old file parked at {}",
                            crate::paths::file_name(&bak)
                        );
                        Self { conn: Some(conn) }
                    }
                    Err(second) => {
                        tracing::warn!(
                            "session db unusable ({first:#} then {second:#}) — persistence off"
                        );
                        Self { conn: None }
                    }
                }
            }
        }
    }

    /// Rewrites the whole session in one transaction — called at app
    /// quit only. Failures log and leave the previous state on disk.
    pub fn save_session(&mut self, tabs: &[Tab], active: usize, next_title: usize) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        match save_inner(conn, tabs, active, next_title) {
            Ok(()) => tracing::info!(tabs = tabs.len(), "session saved"),
            Err(err) => tracing::warn!("session save failed: {err:#}"),
        }
    }

    /// Reads the session back — per-row decode failures skip that tab
    /// rather than losing the others; a wholesale failure yields an
    /// empty session (the next save rewrites everything anyway).
    pub fn load_session(&mut self) -> StoredSession {
        let Some(conn) = self.conn.as_mut() else {
            return StoredSession::default();
        };
        let t0 = std::time::Instant::now();
        match load_inner(conn) {
            Ok(session) => {
                tracing::info!(
                    tabs = session.tabs.len(),
                    ms = t0.elapsed().as_millis(),
                    "session restored"
                );
                session
            }
            Err(err) => {
                tracing::warn!("session restore failed: {err:#}");
                StoredSession::default()
            }
        }
    }

    /// Hard-purges every row of a deleted workspace — a safety net:
    /// its tabs' rows are already gone through `delete_documents` (the
    /// viewer only deletes EMPTY workspaces), this covers rows stranded
    /// by a crash between the viewer close and the drain.
    pub fn delete_workspace(&mut self, id: &str) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        match conn.execute("DELETE FROM documents WHERE workspace_id = ?1", params![id]) {
            Ok(n) if n > 0 => {
                tracing::info!(workspace = %id, removed = n, "workspace rows purged")
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(workspace = %id, "workspace purge failed: {err}"),
        }
    }

    /// Deletes the rows of documents whose tabs were closed — their
    /// records must not come back at the next launch.
    pub fn delete_documents(&mut self, ids: &[String]) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let Ok(mut stmt) = conn
            .prepare("DELETE FROM documents WHERE doc_id = ?1")
            .inspect_err(|err| tracing::warn!("session delete prepare failed: {err}"))
        else {
            return;
        };
        for id in ids {
            if let Err(err) = stmt.execute(params![id]) {
                tracing::warn!(%id, "cannot delete stored capture: {err}");
            }
        }
        // Content is already zeroed by secure_delete; the freed pages
        // are reclaimed by the measured VACUUM at drop — rewriting the
        // whole file on every tab close would be wasteful.
    }

    /// How many captures the store currently holds — the settings
    /// warning asks only when there is something to lose.
    pub fn document_count(&self) -> usize {
        self.conn
            .as_ref()
            .and_then(|conn| {
                conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get::<_, i64>(0))
                    .ok()
            })
            .unwrap_or(0) as usize
    }

    /// Applies the retention policy right now: `Never` empties the
    /// store, `Today` drops every capture taken before this morning.
    /// Called at launch and when the setting is confirmed more
    /// restrictively.
    pub fn apply_retention(&mut self, retention: crate::config::DbRetention) {
        use crate::config::DbRetention as R;
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let removed = match retention {
            R::Always => 0,
            R::Never => {
                if let Err(err) =
                    conn.execute_batch("DELETE FROM documents; DELETE FROM session; VACUUM;")
                {
                    tracing::warn!("session clear failed: {err}");
                }
                return;
            }
            R::Today => conn
                .execute(
                    "DELETE FROM documents WHERE captured_epoch < ?1",
                    params![local_midnight()],
                )
                .unwrap_or(0),
        };
        if removed > 0 {
            tracing::info!(removed, "retention pruned old captures");
        }
    }

    /// Upserts one document row — the eager counterpart of
    /// `save_session`'s per-tab branches. `w.base = Some` runs the full
    /// INSERT (PNG encode included); `None` updates the object/meta
    /// columns only and is an error when the row does not exist yet
    /// (a first write always carries pixels).
    pub fn write_doc(&mut self, w: &DocWrite) -> anyhow::Result<()> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        let saved_at = w.saved_at.as_ref().map(|d| d.to_rfc3339());
        let source_path = w
            .source_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let annotations = postcard::to_allocvec(&w.annotations)?;
        let anno_undo = postcard::to_allocvec(&w.anno_undo)?;
        let anno_redo = postcard::to_allocvec(&w.anno_redo)?;
        match &w.base {
            Some(image) => {
                conn.execute(
                    "INSERT OR REPLACE INTO documents (
                        doc_id, ord, title, saved,
                        captured_at, captured_epoch, saved_at, source_path, password_hash,
                        workspace_id,
                        base_png, scale_factor, monitor_json,
                        next_id, annotations, anno_undo, anno_redo
                    ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                    params![
                        w.id,
                        w.ord,
                        w.title,
                        w.saved,
                        w.captured_at.to_rfc3339(),
                        w.captured_at.timestamp(),
                        saved_at,
                        source_path,
                        w.password_hash,
                        w.workspace_id,
                        ser::png_encode(image)?,
                        w.scale_factor,
                        serde_json::to_string(&w.monitor)?,
                        w.next_id as i64,
                        annotations,
                        anno_undo,
                        anno_redo,
                    ],
                )?;
            }
            None => {
                let n = conn.execute(
                    "UPDATE documents SET
                        ord=?2, title=?3, saved=?4, saved_at=?5,
                        source_path=?6, password_hash=?7,
                        next_id=?8, annotations=?9, anno_undo=?10, anno_redo=?11
                     WHERE doc_id=?1",
                    params![
                        w.id,
                        w.ord,
                        w.title,
                        w.saved,
                        saved_at,
                        source_path,
                        w.password_hash,
                        w.next_id as i64,
                        annotations,
                        anno_undo,
                        anno_redo,
                    ],
                )?;
                anyhow::ensure!(n > 0, "no stored row for {}", w.id);
            }
        }
        Ok(())
    }

    /// Refreshes the meta columns of every listed row — reorders,
    /// renames, save-state flips. No blob is touched; a row that is
    /// not stored yet is skipped (its `DocWrite` carries the values).
    pub fn write_meta(&mut self, rows: &[MetaRow]) -> anyhow::Result<()> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE documents SET
                    ord=?2, title=?3, saved=?4, saved_at=?5,
                    source_path=?6, password_hash=?7
                 WHERE doc_id=?1",
            )?;
            for r in rows {
                let saved_at = r.saved_at.as_ref().map(|d| d.to_rfc3339());
                let source_path = r
                    .source_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned());
                stmt.execute(params![
                    r.id,
                    r.ord,
                    r.title,
                    r.saved,
                    saved_at,
                    source_path,
                    r.password_hash,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Writes the session row (active tab, auto-title counter).
    pub fn set_session(&mut self, active: usize, next_title: usize) -> anyhow::Result<()> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        conn.execute(
            "INSERT OR REPLACE INTO session (id, active, next_title) VALUES (0, ?1, ?2)",
            params![active as i64, next_title as i64],
        )?;
        Ok(())
    }

    /// The quit-time barrier: refresh every meta column, the session
    /// row, and drop any stored row whose document is gone (covers a
    /// delete that failed mid-session). Queued after the eager jobs so
    /// it only ever polishes what they wrote.
    pub fn finalize(
        &mut self,
        rows: &[MetaRow],
        live: &[String],
        active: usize,
        next_title: usize,
    ) -> anyhow::Result<()> {
        if self.conn.is_none() {
            return Ok(());
        }
        self.write_meta(rows)?;
        self.set_session(active, next_title)?;
        let live: std::collections::HashSet<&str> = live.iter().map(String::as_str).collect();
        let stored: Vec<String> = {
            let conn = self.conn.as_mut().expect("checked above");
            let mut s = conn.prepare("SELECT doc_id FROM documents")?;
            let rows = s.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let conn = self.conn.as_mut().expect("checked above");
        for id in stored {
            if !live.contains(id.as_str()) {
                conn.execute("DELETE FROM documents WHERE doc_id = ?1", params![id])?;
            }
        }
        Ok(())
    }

    /// Empties the store — the session was deliberately discarded, so
    /// nothing of it may come back at the next launch. Discard is a
    /// single rare action: the VACUUM runs right here.
    pub fn clear(&mut self) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if let Err(err) = conn.execute_batch("DELETE FROM documents; DELETE FROM session; VACUUM;")
        {
            tracing::warn!("session clear failed: {err}");
        }
    }
}

impl Store {
    /// Reclaims space freed by deletions — MEASURED, not flagged: the
    /// VACUUM only runs when the freelist holds > 32 MiB AND > 50 % of
    /// the file. Called on the writer thread at quit (after the queue
    /// drained, outside any transaction); failures log and never block
    /// the exit.
    pub fn vacuum_if_needed(&mut self) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let sizes = |conn: &Connection| -> (i64, i64, i64) {
            let get = |p: &str| -> i64 {
                conn.pragma_query_value(None, p, |r| r.get::<_, i64>(0))
                    .unwrap_or(0)
            };
            (get("page_count"), get("freelist_count"), get("page_size"))
        };
        let (pages, free, psize) = sizes(conn);
        if free * psize <= 32 * 1024 * 1024 || free * 2 <= pages {
            return;
        }
        tracing::info!(
            bytes = pages * psize,
            free_bytes = free * psize,
            "session db vacuum"
        );
        if let Err(err) = conn.execute_batch("VACUUM") {
            tracing::warn!("session vacuum failed: {err}");
            return;
        }
        // In WAL a VACUUM can leave the reclaimed space parked in the
        // -wal side file: truncate it back.
        let wal = conn
            .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
            .is_ok_and(|m| m.eq_ignore_ascii_case("wal"));
        if wal && let Err(err) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
            tracing::warn!("session wal checkpoint failed: {err}");
        }
        let (pages, free, psize) = sizes(conn);
        tracing::info!(
            bytes = pages * psize,
            free_bytes = free * psize,
            "session db vacuumed"
        );
    }
}

impl Drop for Store {
    /// Safety net when the writer thread exits normally — a no-op once
    /// `vacuum_if_needed` already ran (the freelist is empty then).
    fn drop(&mut self) {
        self.vacuum_if_needed();
    }
}

/// Local midnight, epoch seconds — the `Today` retention cutoff.
fn local_midnight() -> i64 {
    let today = chrono::Local::now().date_naive();
    today
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

/// `data_dir()/session.db` — the location comes from `crate::paths`,
/// the single resolver (portable exe-adjacent dir on Windows,
/// Application Support on macOS, `PIXME_DATA_DIR` in dev-tools only).
pub(crate) fn db_path() -> PathBuf {
    crate::paths::db_path()
}

/// `<db>.bak` — where a corrupt or foreign-version file is parked.
fn bak_path(path: &Path) -> PathBuf {
    let mut bak = path.as_os_str().to_os_string();
    bak.push(".bak");
    PathBuf::from(bak)
}

/// Deletes a `.bak` recovery copy older than [`BAK_KEEP_DAYS`]: it
/// holds captures the user may recover by hand for a while, but must
/// not linger forever.
fn prune_old_backup(path: &Path) {
    let bak = bak_path(path);
    let Ok(meta) = std::fs::metadata(&bak) else {
        return;
    };
    let aged = meta
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|a| a > std::time::Duration::from_secs(BAK_KEEP_DAYS * 86_400));
    if aged {
        match std::fs::remove_file(&bak) {
            Ok(()) => tracing::info!(
                "aged session backup removed: {}",
                crate::paths::file_name(&bak)
            ),
            Err(err) => tracing::warn!("cannot remove {}: {err}", crate::paths::file_name(&bak)),
        }
    }
}

/// Opens the file and brings its schema to [`SCHEMA_VERSION`].
fn try_open(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    migrate(&conn)?;
    Ok(conn)
}

/// `true` when `table` already has a `column` — makes the v14 → v15
/// ALTER idempotent so a re-run can never fail into the `.bak` path.
fn column_exists(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.iter().any(|n| n == column))
}

/// Brings `conn`'s schema to [`SCHEMA_VERSION`]: unknown versions lose
/// their data tables and are rebuilt (the session is a convenience,
/// not an archive) — EXCEPT v14, which is upgraded in place: it only
/// lacks `workspace_id`, so the column is ALTERed on (every existing
/// capture takes the `main` default) rather than wiped.
fn migrate(conn: &Connection) -> anyhow::Result<()> {
    // Deleted payload bytes are zeroed rather than left in free pages —
    // captures may hold sensitive content.
    conn.execute_batch("PRAGMA secure_delete = ON;")?;
    conn.execute_batch(DDL_META)?;
    let version: i64 = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    match version {
        SCHEMA_VERSION => {}
        14 => {
            if !column_exists(conn, "documents", "workspace_id")? {
                conn.execute_batch(
                    "ALTER TABLE documents ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'main'",
                )?;
            }
            tracing::info!("session db migrated 14 → 15 (workspace_id column)");
        }
        _ => {
            // Unknown layout: drop the data tables (not meta itself — it is
            // recreated by DDL above) and rebuild at the current version.
            conn.execute_batch("DROP TABLE IF EXISTS documents; DROP TABLE IF EXISTS session;")?;
        }
    }
    conn.execute_batch(DDL_TABLES)?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS documents_workspace_idx ON documents (workspace_id)",
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// The transaction body of [`Store::save_session`]. Incremental per
/// tab: unchanged tabs (`persisted_rev == Some(doc.revision)`) touch
/// only their `ord`/meta columns; annotation-only edits update
/// `annotations` + history without re-encoding `base_png`; only a new
/// or pixel-mutated tab (`base_dirty`) runs `png_encode`.
fn save_inner(
    conn: &mut Connection,
    tabs: &[Tab],
    active: usize,
    next_title: usize,
) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO session (id, active, next_title) VALUES (0, ?1, ?2)",
        params![active as i64, next_title as i64],
    )?;
    // Prune any row whose tab is not in the live list anymore (covers
    // tabs closed on a path that skipped `delete_documents`).
    let live: std::collections::HashSet<&str> =
        tabs.iter().map(|t| t.doc.meta.id.as_str()).collect();
    let stored: Vec<String> = {
        let mut s = tx.prepare("SELECT doc_id FROM documents")?;
        let rows = s.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in stored {
        if !live.contains(id.as_str()) {
            tx.execute("DELETE FROM documents WHERE doc_id = ?1", params![id])?;
        }
    }
    {
        let mut meta_only = tx.prepare(
            "UPDATE documents SET
                ord=?2, title=?3, saved=?4, saved_at=?5,
                source_path=?6, password_hash=?7
             WHERE doc_id=?1",
        )?;
        let mut anno_only = tx.prepare(
            "UPDATE documents SET
                ord=?2, title=?3, saved=?4, saved_at=?5,
                source_path=?6, password_hash=?7,
                next_id=?8, annotations=?9, anno_undo=?10, anno_redo=?11
             WHERE doc_id=?1",
        )?;
        let mut full = tx.prepare(
            "INSERT OR REPLACE INTO documents (
                doc_id, ord, title, saved,
                captured_at, captured_epoch, saved_at, source_path, password_hash,
                workspace_id,
                base_png, scale_factor, monitor_json,
                next_id, annotations, anno_undo, anno_redo
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        )?;
        for (ord, tab) in tabs.iter().enumerate() {
            let doc = &tab.doc;
            let saved_at = doc.meta.saved_at.as_ref().map(|d| d.to_rfc3339());
            let source_path = doc
                .meta
                .source_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned());
            if !tab.base_dirty && tab.persisted_rev == Some(doc.revision) {
                let n = meta_only.execute(params![
                    doc.meta.id,
                    ord as i64,
                    tab.title,
                    tab.saved,
                    saved_at,
                    source_path,
                    doc.meta.password_hash,
                ])?;
                if n > 0 {
                    continue;
                }
            } else if !tab.base_dirty && tab.persisted_rev.is_some() {
                let n = anno_only.execute(params![
                    doc.meta.id,
                    ord as i64,
                    tab.title,
                    tab.saved,
                    saved_at,
                    source_path,
                    doc.meta.password_hash,
                    doc.next_id() as i64,
                    postcard::to_allocvec(&doc.annotations)?,
                    postcard::to_allocvec(&tab.anno_undo)?,
                    postcard::to_allocvec(&tab.anno_redo)?,
                ])?;
                if n > 0 {
                    continue;
                }
            }
            full.execute(params![
                doc.meta.id,
                ord as i64,
                tab.title,
                tab.saved,
                doc.meta.captured_at.to_rfc3339(),
                doc.meta.captured_at.timestamp(),
                saved_at,
                source_path,
                doc.meta.password_hash,
                doc.meta.workspace_id,
                ser::png_encode(&doc.base.image)?,
                doc.base.scale_factor,
                serde_json::to_string(&doc.base.monitor)?,
                doc.next_id() as i64,
                postcard::to_allocvec(&doc.annotations)?,
                postcard::to_allocvec(&tab.anno_undo)?,
                postcard::to_allocvec(&tab.anno_redo)?,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// A raw row as read back — decoding happens in [`RawDoc::into_tab`]
/// so one bad row costs one tab, not the session.
struct RawDoc {
    doc_id: String,
    title: String,
    saved: bool,
    captured_at: String,
    saved_at: Option<String>,
    source_path: Option<String>,
    password_hash: Option<String>,
    workspace_id: String,
    base_png: Vec<u8>,
    scale_factor: f32,
    monitor_json: String,
    next_id: u64,
    annotations: Vec<u8>,
    anno_undo: Vec<u8>,
    anno_redo: Vec<u8>,
}

impl RawDoc {
    /// Decodes every blob of the row into a live [`StoredTab`].
    fn into_tab(self) -> anyhow::Result<StoredTab> {
        let t_all = std::time::Instant::now();
        let doc_id = self.doc_id.clone();
        let png_kb = self.base_png.len() as f64 / 1024.0;
        let t_png = std::time::Instant::now();
        let image = ser::png_decode(&self.base_png)?;
        let png_ms = t_png.elapsed();
        let captured_at = chrono::DateTime::parse_from_rfc3339(&self.captured_at)
            .map(|d| d.with_timezone(&chrono::Local))?;
        let saved_at = self
            .saved_at
            .map(|s| {
                chrono::DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&chrono::Local))
            })
            .transpose()?;
        let mut doc = Document::new(crate::capture::Capture {
            image: std::sync::Arc::new(image),
            scale_factor: self.scale_factor,
            monitor: serde_json::from_str(&self.monitor_json)?,
        });
        doc.meta = CaptureMeta {
            id: self.doc_id,
            captured_at,
            saved_at,
            source_path: self.source_path.map(PathBuf::from),
            password_hash: self.password_hash,
            // An empty/unknown id resolves to `main` downstream — the
            // viewer re-checks against the live workspace list anyway.
            workspace_id: if self.workspace_id.is_empty() {
                crate::config::workspace::MAIN_ID.to_owned()
            } else {
                self.workspace_id
            },
        };
        doc.annotations = postcard::from_bytes(&self.annotations)?;
        doc.set_next_id(self.next_id);
        doc.revision += 1;
        tracing::debug!(
            doc_id,
            png_kb,
            png_ms = png_ms.as_millis(),
            row_ms = t_all.elapsed().as_millis(),
            "session capture decoded"
        );
        Ok(StoredTab {
            title: self.title,
            saved: self.saved,
            doc,
            anno_undo: postcard::from_bytes(&self.anno_undo)?,
            anno_redo: postcard::from_bytes(&self.anno_redo)?,
        })
    }
}

/// The query body of [`Store::load_session`].
fn load_inner(conn: &Connection) -> anyhow::Result<StoredSession> {
    let (active, next_title) = conn
        .query_row(
            "SELECT active, next_title FROM session WHERE id = 0",
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )
        .optional()?
        .unwrap_or((0, 0));
    let mut stmt = conn.prepare(
        "SELECT doc_id, title, saved, captured_at, saved_at, source_path,
                password_hash, workspace_id, base_png, scale_factor, monitor_json,
                next_id, annotations, anno_undo, anno_redo
         FROM documents ORDER BY ord",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RawDoc {
                doc_id: row.get(0)?,
                title: row.get(1)?,
                saved: row.get(2)?,
                captured_at: row.get(3)?,
                saved_at: row.get(4)?,
                source_path: row.get(5)?,
                password_hash: row.get(6)?,
                workspace_id: row.get(7)?,
                base_png: row.get(8)?,
                scale_factor: row.get(9)?,
                monitor_json: row.get(10)?,
                next_id: row.get::<_, i64>(11)? as u64,
                annotations: row.get(12)?,
                anno_undo: row.get(13)?,
                anno_redo: row.get(14)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut tabs = Vec::with_capacity(rows.len());
    for raw in rows {
        let doc_id = raw.doc_id.clone();
        match raw.into_tab() {
            Ok(tab) => tabs.push(tab),
            Err(err) => tracing::warn!(doc_id, "skipping unrestorable capture: {err:#}"),
        }
    }
    Ok(StoredSession {
        tabs,
        active,
        next_title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, MonitorInfo, PhysicalRect};
    use crate::editor::document::{Annotation, AnnotationKind, Dash, RectObj};
    use crate::ui::viewer::Viewer;
    use egui::Color32;

    /// A unique path under the OS temp dir — each test gets its own file.
    fn tmp_db() -> PathBuf {
        std::env::temp_dir().join(format!(
            "pixme-store-test-{}.db",
            crate::editor::meta::new_id()
        ))
    }

    /// Pushes a `w`×`h` solid-colour capture tab with `title`; returns
    /// the tab index for post-push tweaking.
    fn push_tab(viewer: &mut Viewer, title: &str, w: u32, color: [u8; 4]) -> usize {
        let doc = Document::new(Capture {
            image: std::sync::Arc::new(image::RgbaImage::from_pixel(w, w, image::Rgba(color))),
            scale_factor: 2.0,
            monitor: MonitorInfo {
                id: 7,
                name: "Built-in".to_owned(),
                rect: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: w / 2,
                    height: w / 2,
                },
                scale_factor: 2.0,
                is_primary: true,
            },
        });
        viewer.push_named_document(doc, title.to_owned(), false);
        viewer.tabs.len() - 1
    }

    fn rect_anno(id: u64) -> Annotation {
        Annotation {
            id,
            kind: AnnotationKind::Rect(RectObj {
                rect: PhysicalRect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4,
                },
                stroke: Color32::RED,
                stroke_w: 2.0,
                dash: Dash::Dash,
                fill: Some(Color32::BLUE),
                radius: 4.0,
                outline: 0.0,
                outline_color: Color32::WHITE,
                opacity: 0.5,
                shadow: 3.0,
            }),
            tool: None,
        }
    }

    /// Saves `viewer` into `store` exactly like the app quit path.
    fn save(store: &mut Store, viewer: &Viewer) {
        store.save_session(
            &viewer.tabs,
            viewer.active_index().unwrap_or(0),
            viewer.next_title(),
        );
    }

    /// A tab, its annotations, meta and undo/redo stacks all come back.
    #[test]
    fn round_trip_restores_everything() {
        let path = tmp_db();
        let mut store = Store::open_at(path.clone());
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "one", 8, [10, 20, 30, 255]);
        let j = push_tab(&mut viewer, "two", 4, [200, 0, 0, 255]);
        {
            let tab = &mut viewer.tabs[j];
            let id = tab.doc.alloc_id();
            tab.doc.annotations.push(rect_anno(id));
            tab.doc.set_next_id(9);
            tab.doc.meta.set_password("secret");
            tab.doc.meta.source_path = Some(PathBuf::from("/tmp/shot.png"));
            tab.doc.meta.saved_at = Some(chrono::Local::now());
            tab.saved = true;
            tab.anno_undo.push_back(AnnoEdit::Add {
                anno: rect_anno(id),
                index: 0,
            });
            tab.anno_redo.push_back(AnnoEdit::Remove {
                anno: rect_anno(id),
                index: 0,
            });
        }
        viewer.recompute_dirty();
        save(&mut store, &viewer);
        drop(store);

        let mut store = Store::open_at(path.clone());
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 2);
        assert_eq!(session.active, viewer.active);
        let tab = &session.tabs[1];
        assert_eq!(tab.title, "two");
        assert!(tab.saved);
        assert_eq!(tab.doc.annotations.len(), 1);
        assert_eq!(tab.doc.next_id(), 9);
        assert_eq!(tab.doc.base.image.dimensions(), (4, 4));
        assert_eq!(tab.doc.base.image.get_pixel(0, 0).0, [200, 0, 0, 255]);
        assert_eq!(tab.doc.base.scale_factor, 2.0);
        assert_eq!(tab.doc.base.monitor.id, 7);
        assert!(tab.doc.meta.is_protected());
        assert!(tab.doc.meta.check_password("secret"));
        assert!(!tab.doc.meta.check_password("nope"));
        assert_eq!(
            tab.doc.meta.source_path.as_deref(),
            Some(Path::new("/tmp/shot.png"))
        );
        assert!(tab.doc.meta.saved_at.is_some());
        assert_eq!(tab.anno_undo.len(), 1);
        assert_eq!(tab.anno_redo.len(), 1);
        assert_eq!(session.next_title, viewer.next_title());
        let _ = std::fs::remove_file(path);
    }

    /// A tab closed mid-session loses its row at once — the next load
    /// brings back only the surviving captures.
    #[test]
    fn delete_documents_drops_closed_tabs() {
        let path = tmp_db();
        let mut store = Store::open_at(path.clone());
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "a", 4, [0, 0, 0, 255]);
        push_tab(&mut viewer, "b", 4, [0, 0, 0, 255]);
        push_tab(&mut viewer, "c", 4, [0, 0, 0, 255]);
        save(&mut store, &viewer);
        let gone = viewer.tabs[1].doc.meta.id.clone();
        store.delete_documents(&[gone]);
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 2);
        assert_eq!(session.tabs[0].title, "a");
        assert_eq!(session.tabs[1].title, "c");
        let _ = std::fs::remove_file(path);
    }

    /// The viewer's close path records doc ids for the store drain.
    #[test]
    fn close_tabs_records_ids_for_the_store() {
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "a", 4, [0, 0, 0, 255]);
        push_tab(&mut viewer, "b", 4, [0, 0, 0, 255]);
        let b_id = viewer.tabs[1].doc.meta.id.clone();
        viewer.close_tabs(&[1]);
        assert_eq!(viewer.take_closed_ids(), vec![b_id]);
        assert!(viewer.take_closed_ids().is_empty(), "drained once");
    }

    /// Discard: a cleared store restores nothing.
    #[test]
    fn clear_empties_the_session() {
        let path = tmp_db();
        let mut store = Store::open_at(path.clone());
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "a", 4, [0, 0, 0, 255]);
        save(&mut store, &viewer);
        store.clear();
        assert!(store.load_session().tabs.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// A garbage file is parked as `.bak` and a fresh store takes over.
    #[test]
    fn corrupt_file_is_parked_and_rebuilt() {
        let path = tmp_db();
        std::fs::write(&path, b"not a sqlite database at all").unwrap();
        let mut store = Store::open_at(path.clone());
        assert!(store.load_session().tabs.is_empty());
        let bak = {
            let mut s = path.clone().into_os_string();
            s.push(".bak");
            PathBuf::from(s)
        };
        assert!(bak.exists(), "corrupt file parked aside");
        // The rebuilt store works right away.
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "a", 4, [0, 0, 0, 255]);
        save(&mut store, &viewer);
        assert_eq!(store.load_session().tabs.len(), 1);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(bak);
    }

    /// A database at another schema version is wiped and rebuilt —
    /// forward and backward compatibility is "start empty".
    #[test]
    fn foreign_schema_version_is_wiped() {
        let path = tmp_db();
        Store::open_at(path.clone()); // creates a v1 file with a row
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "a", 4, [0, 0, 0, 255]);
        Store::open_at(path.clone()).save_session(&viewer.tabs, 0, 1);
        // Downgrade the declared version, keep the data row.
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE meta SET value = '99'", []).unwrap();
        drop(conn);
        let mut store = Store::open_at(path.clone());
        assert!(store.load_session().tabs.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// One corrupt blob loses only its own tab — neighbours survive.
    #[test]
    fn a_bad_row_costs_only_its_tab() {
        let path = tmp_db();
        let mut store = Store::open_at(path.clone());
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "good", 4, [0, 0, 0, 255]);
        push_tab(&mut viewer, "bad", 4, [0, 0, 0, 255]);
        save(&mut store, &viewer);
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE documents SET annotations = X'ff' WHERE title = 'bad'",
            [],
        )
        .unwrap();
        drop(conn);
        let mut store = Store::open_at(path.clone());
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.tabs[0].title, "good");
        let _ = std::fs::remove_file(path);
    }

    /// Restore marks the session dirty iff some tab was unsaved, and a
    /// protected tab comes back locked (`unlocked` is never persisted).
    #[test]
    fn restore_recomputes_dirty_and_relocks() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "open", 4, [0, 0, 0, 255]);
        let j = push_tab(&mut viewer, "locked", 4, [0, 0, 0, 255]);
        viewer.tabs[j].doc.meta.set_password("secret");
        viewer.recompute_dirty();
        save(&mut store, &viewer);
        let session = store.load_session();

        let mut restored = Viewer::default();
        restored.restore_session(session, None);
        assert!(restored.dirty, "an unsaved tab keeps the session dirty");
        assert!(restored.session_restored());
        assert!(
            restored.tabs[1].doc.meta.is_protected() && !restored.tabs[1].unlocked,
            "protected tab restores locked"
        );
        assert!(!restored.tabs[0].doc.meta.is_protected());
        assert_eq!(restored.active_index(), Some(viewer.active));
    }
    /// Golden serialization: the postcard layout of a fixed annotation
    /// set + undo/redo stacks must stay byte-identical. ANY drift means
    /// the on-disk format changed — bump SCHEMA_VERSION in this file and
    /// regenerate EXPECTED after reviewing the change (the message names
    /// it on failure).
    #[test]
    fn postcard_layout_is_golden() {
        let annotations = vec![
            rect_anno(1),
            Annotation {
                id: 2,
                kind: AnnotationKind::Step(super::super::editor::document::StepObj {
                    pos: egui::pos2(7.5, 8.5),
                    count: 3,
                    mode: super::super::editor::document::StepMode::Numbers,
                    style: super::super::editor::document::StepStyle::Square,
                    size: super::super::editor::document::StepSize::Large,
                    bg: Color32::from_rgb(10, 20, 30),
                    fg: Color32::WHITE,
                    ring: 2.0,
                    ring_color: Color32::WHITE,
                    opacity: 0.75,
                    shadow: 1.5,
                    lower: false,
                }),
                tool: None,
            },
            Annotation {
                id: 3,
                kind: AnnotationKind::Emoji(super::super::editor::document::EmojiObj {
                    rect: PhysicalRect {
                        x: 10,
                        y: 20,
                        width: 48,
                        height: 48,
                    },
                    id: 0x1F449,
                }),
                tool: None,
            },
        ];
        let undo: Vec<AnnoEdit> = vec![
            AnnoEdit::Add {
                anno: rect_anno(4),
                index: 2,
            },
            AnnoEdit::Replace {
                before: rect_anno(5),
                after: rect_anno(6),
            },
        ];
        let redo: Vec<AnnoEdit> = vec![AnnoEdit::Remove {
            anno: rect_anno(7),
            index: 0,
        }];
        let blob = [
            postcard::to_allocvec(&annotations).unwrap(),
            postcard::to_allocvec(&undo).unwrap(),
            postcard::to_allocvec(&redo).unwrap(),
        ]
        .concat();
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, EXPECTED,
            "postcard layout changed — bump SCHEMA_VERSION in              src/store/mod.rs and regenerate EXPECTED after review"
        );
    }

    /// See `postcard_layout_is_golden`.
    const EXPECTED: &str = "03010002040304ff0000ff0000004001010000ffff0000804000000000ffffffff0000003f000040400002070000f04000000841030001020a141effffffffff00000040ffffffff0000403f0000c03f0000030b14283030c9e807000200040002040304ff0000ff0000004001010000ffff0000804000000000ffffffff0000003f00004040000202050002040304ff0000ff0000004001010000ffff0000804000000000ffffffff0000003f0000404000060002040304ff0000ff0000004001010000ffff0000804000000000ffffffff0000003f00004040000101070002040304ff0000ff0000004001010000ffff0000804000000000ffffffff0000003f000040400000";

    /// An emoji annotation survives a full session save/load — the doc
    /// stores the id only, pixels come back from the embedded set.
    #[test]
    fn emoji_annotation_roundtrips_session() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        let idx = push_tab(&mut viewer, "emoji-tab", 16, [10, 20, 30, 255]);
        viewer.tabs[idx].doc.annotations.push(Annotation {
            id: 42,
            kind: AnnotationKind::Emoji(super::super::editor::document::EmojiObj {
                rect: PhysicalRect {
                    x: 4,
                    y: 4,
                    width: 24,
                    height: 24,
                },
                id: 0x1F525,
            }),
            tool: None,
        });
        save(&mut store, &viewer);
        let restored = store.load_session();
        assert_eq!(restored.tabs.len(), 1);
        let doc = &restored.tabs[0].doc;
        assert_eq!(doc.annotations.len(), 1);
        match &doc.annotations[0].kind {
            AnnotationKind::Emoji(o) => {
                assert_eq!(o.id, 0x1F525);
                assert_eq!(
                    o.rect,
                    PhysicalRect {
                        x: 4,
                        y: 4,
                        width: 24,
                        height: 24
                    }
                );
            }
            other => panic!("expected Emoji, got {other:?}"),
        }
    }
    /// `Today` keeps only captures taken this morning or later;
    /// `Never` empties the store and `document_count` reports them.
    #[test]
    fn retention_prunes_older_and_never_clears() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        let old = push_tab(&mut viewer, "old", 4, [0, 0, 0, 255]);
        viewer.tabs[old].doc.meta.captured_at = chrono::Local::now() - chrono::Duration::days(2);
        push_tab(&mut viewer, "new", 4, [0, 0, 0, 255]);
        save(&mut store, &viewer);
        assert_eq!(store.document_count(), 2);
        store.apply_retention(crate::config::DbRetention::Today);
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.tabs[0].title, "new");
        store.apply_retention(crate::config::DbRetention::Never);
        assert_eq!(store.document_count(), 0);
    }

    /// A restored tab whose base image was not mutated (`base_dirty ==
    /// false`) must not have its `base_png` re-written — even when an
    /// annotation edit bumps `doc.revision`.
    #[test]
    fn incremental_save_skips_unchanged_base_png() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "one", 4, [10, 20, 30, 255]);
        push_tab(&mut viewer, "two", 4, [40, 50, 60, 255]);
        save(&mut store, &viewer);

        // Restore: both tabs get `base_dirty = false` and a persisted_rev.
        let mut viewer = Viewer::default();
        viewer.restore_session(store.load_session(), None);
        // Poison the in-memory pixels — if `save_session` re-encoded
        // them, the next load would come back red instead of the stored
        // blue/green.
        for t in &mut viewer.tabs {
            std::sync::Arc::make_mut(&mut t.doc.base.image).put_pixel(
                0,
                0,
                image::Rgba([255, 0, 0, 255]),
            );
        }
        // Tab 0 stays untouched; tab 1 gets an annotation edit (revision
        // moves, `base_dirty` stays false) + a rename.
        viewer.tabs[1].title = "two-renamed".to_owned();
        viewer.tabs[1].doc.revision += 1;
        save(&mut store, &viewer);

        let back = store.load_session();
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(
            back.tabs[0].doc.base.image.get_pixel(0, 0).0,
            [10, 20, 30, 255]
        );
        assert_eq!(
            back.tabs[1].doc.base.image.get_pixel(0, 0).0,
            [40, 50, 60, 255]
        );
        assert_eq!(back.tabs[1].title, "two-renamed");
    }

    /// v14 files keep their captures: only `workspace_id` is missing
    /// there, so the column is ALTERed on (every row takes `main`)
    /// instead of the file being parked as `.bak` and rebuilt empty.
    #[test]
    fn v14_upgrades_in_place_without_bak() {
        let path = tmp_db();
        {
            let mut store = Store::open_at(path.clone());
            let mut viewer = Viewer::default();
            push_tab(&mut viewer, "kept", 4, [1, 2, 3, 255]);
            save(&mut store, &viewer);
        }
        // Reshape the file as its v14 self: no column, no index, the
        // declared version rolled back.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX documents_workspace_idx;
             ALTER TABLE documents DROP COLUMN workspace_id;
             UPDATE meta SET value = '14' WHERE key = 'schema_version';",
        )
        .unwrap();
        drop(conn);

        let mut store = Store::open_at(path.clone());
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 1, "the v14 capture survives");
        assert_eq!(session.tabs[0].title, "kept");
        assert_eq!(
            session.tabs[0].doc.meta.workspace_id,
            crate::config::workspace::MAIN_ID,
            "pre-workspaces captures belong to 'main'"
        );
        let bak = {
            let mut s = path.clone().into_os_string();
            s.push(".bak");
            PathBuf::from(s)
        };
        assert!(!bak.exists(), "a v14 file is upgraded, never parked");
        let _ = std::fs::remove_file(path);
    }

    /// Workspace ids ride the document row verbatim — the SQL column,
    /// never the postcard payloads.
    #[test]
    fn workspace_id_round_trips() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        let a = push_tab(&mut viewer, "a", 4, [0, 0, 0, 255]);
        let b = push_tab(&mut viewer, "b", 4, [0, 0, 0, 255]);
        viewer.tabs[a].doc.meta.workspace_id = "ws-a".to_owned();
        viewer.tabs[b].doc.meta.workspace_id = "ws-b".to_owned();
        save(&mut store, &viewer);
        let session = store.load_session();
        assert_eq!(session.tabs[0].doc.meta.workspace_id, "ws-a");
        assert_eq!(session.tabs[1].doc.meta.workspace_id, "ws-b");
    }

    /// Deleting a workspace hard-purges ITS rows only — `main` keeps
    /// its captures (the viewer only ever offers deletion on empty
    /// workspaces; this is the safety net for stranded rows).
    #[test]
    fn delete_workspace_purges_only_its_rows() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        push_tab(&mut viewer, "keep", 4, [0, 0, 0, 255]);
        let gone = push_tab(&mut viewer, "gone", 4, [0, 0, 0, 255]);
        viewer.tabs[gone].doc.meta.workspace_id = "doomed".to_owned();
        save(&mut store, &viewer);
        store.delete_workspace("doomed");
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.tabs[0].title, "keep");
    }

    /// Retention is GLOBAL: a `Today` prune drops old rows whatever
    /// workspace they belong to — the new column must not scope it.
    #[test]
    fn retention_stays_global_across_workspaces() {
        let mut store = Store::open_in_memory();
        let mut viewer = Viewer::default();
        let old_a = push_tab(&mut viewer, "old-a", 4, [0, 0, 0, 255]);
        let old_b = push_tab(&mut viewer, "old-b", 4, [0, 0, 0, 255]);
        let new = push_tab(&mut viewer, "new", 4, [0, 0, 0, 255]);
        viewer.tabs[old_a].doc.meta.workspace_id = "ws-a".to_owned();
        viewer.tabs[old_b].doc.meta.workspace_id = "ws-b".to_owned();
        viewer.tabs[new].doc.meta.workspace_id = "ws-b".to_owned();
        for i in [old_a, old_b] {
            viewer.tabs[i].doc.meta.captured_at = chrono::Local::now() - chrono::Duration::days(2);
        }
        save(&mut store, &viewer);
        store.apply_retention(crate::config::DbRetention::Today);
        let session = store.load_session();
        assert_eq!(session.tabs.len(), 1, "every workspace is pruned");
        assert_eq!(session.tabs[0].title, "new");
    }
}
