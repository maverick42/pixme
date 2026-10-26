//! Session persistence: the viewer's tabs mirror into a small SQLite
//! database so a normal quit preserves every capture — document,
//! annotations and the chronological undo/redo stacks — and the next
//! viewer open restores them silently.
//!
//! The store only sees writes at three points: app quit (one
//! transaction rewrites the whole session), tab close (that document's
//! row is deleted at once) and session discard (the store is emptied).
//! A crash or kill between writes loses what changed — an accepted
//! trade-off: PixMe stays a resident app with no background writing.
//!
//! One row per tab; per row, three postcard blobs (`annotations`,
//! `anno_undo`, `anno_redo`) hold the object model and its history —
//! bitmaps inside them ride as PNG bytes via [`ser::rgba_png`]. The
//! pixel-level ops (Resize) are NOT persisted: their payloads are
//! whole pre-op images, far too heavy for a small database.

pub mod ser;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::editor::Document;
use crate::editor::history::AnnoEdit;
use crate::editor::meta::CaptureMeta;
use crate::ui::viewer::Tab;

/// Current layout — a file carrying another version is wiped clean
/// rather than migrated (the session is a convenience, not an archive).
const SCHEMA_VERSION: i64 = 9;

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
        ord           INTEGER PRIMARY KEY,
        doc_id        TEXT    NOT NULL,
        title         TEXT    NOT NULL,
        saved         INTEGER NOT NULL,
        captured_at   TEXT    NOT NULL,
        captured_epoch INTEGER NOT NULL DEFAULT 0,
        saved_at      TEXT,
        source_path   TEXT,
        password_hash TEXT,
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

/// The session database — a `None` connection means the file could not
/// be opened at all and persistence is silently off (a resident app
/// must never fail to launch over a store problem).
pub struct Store {
    conn: Option<Connection>,
    /// Deletions happened since open — a `VACUUM` at drop reclaims the
    /// pages `secure_delete` already zeroed.
    needs_vacuum: bool,
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
        Self {
            conn: Some(conn),
            needs_vacuum: false,
        }
    }

    /// Opens (and migrates) the database at `path`. A file that fails to
    /// open or migrate once is moved aside to `<name>.bak` and rebuilt;
    /// a second failure leaves persistence disabled, logged.
    pub fn open_at(path: PathBuf) -> Self {
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("cannot create {}: {err}", dir.display());
        }
        match try_open(&path) {
            Ok(conn) => {
                prune_old_backup(&path);
                Self {
                    conn: Some(conn),
                    needs_vacuum: false,
                }
            }
            Err(first) => {
                let bak = bak_path(&path);
                if let Err(err) = std::fs::rename(&path, &bak) {
                    tracing::warn!("cannot park bad session db as {}: {err}", bak.display());
                }
                match try_open(&path) {
                    Ok(conn) => {
                        tracing::warn!(
                            "session db rebuilt ({first:#}); old file parked at {}",
                            bak.display()
                        );
                        Self {
                            conn: Some(conn),
                            needs_vacuum: false,
                        }
                    }
                    Err(second) => {
                        tracing::warn!(
                            "session db unusable ({first:#} then {second:#}) — persistence off"
                        );
                        Self {
                            conn: None,
                            needs_vacuum: false,
                        }
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
        match load_inner(conn) {
            Ok(session) => {
                tracing::info!(tabs = session.tabs.len(), "session restored");
                session
            }
            Err(err) => {
                tracing::warn!("session restore failed: {err:#}");
                StoredSession::default()
            }
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
        // Content is already zeroed by secure_delete; the VACUUM that
        // reclaims the freed pages runs at drop — rewriting the whole
        // file on every tab close would be wasteful.
        self.needs_vacuum |= !ids.is_empty();
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
                self.needs_vacuum = false;
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
            self.needs_vacuum = true;
        }
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
        self.needs_vacuum = false;
    }
}

impl Drop for Store {
    /// Reclaims the pages freed by per-tab deletions (`secure_delete`
    /// already zeroed their content) — skipped when nothing was
    /// deleted, so a plain quit does not rewrite the file.
    fn drop(&mut self) {
        if self.needs_vacuum
            && let Some(conn) = self.conn.take()
            && let Err(err) = conn.execute_batch("VACUUM")
        {
            tracing::warn!("session vacuum failed: {err}");
        }
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

/// `data_dir()/session.db` (per-user application data); the bare
/// fallback never silently fails to persist.
fn db_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "pixme").map_or_else(
        || PathBuf::from("session.db"),
        |dirs| dirs.data_dir().join("session.db"),
    )
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
            Ok(()) => tracing::info!("aged session backup removed: {}", bak.display()),
            Err(err) => tracing::warn!("cannot remove {}: {err}", bak.display()),
        }
    }
}

/// Opens the file and brings its schema to [`SCHEMA_VERSION`].
fn try_open(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Brings `conn`'s schema to [`SCHEMA_VERSION`]: unknown versions lose
/// their data tables and are rebuilt (the session is a convenience,
/// not an archive).
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
    if version != SCHEMA_VERSION {
        // Unknown layout: drop the data tables (not meta itself — it is
        // recreated by DDL above) and rebuild at the current version.
        conn.execute_batch("DROP TABLE IF EXISTS documents; DROP TABLE IF EXISTS session;")?;
    }
    conn.execute_batch(DDL_TABLES)?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// The transaction body of [`Store::save_session`]. Every tab is
/// persisted — the app never drops captures for size reasons (user
/// decision: the database stays modest in real use, and deleting
/// captures unilaterally is unacceptable).
fn save_inner(
    conn: &mut Connection,
    tabs: &[Tab],
    active: usize,
    next_title: usize,
) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch("DELETE FROM documents; DELETE FROM session;")?;
    tx.execute(
        "INSERT INTO session (id, active, next_title) VALUES (0, ?1, ?2)",
        params![active as i64, next_title as i64],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO documents (
                ord, doc_id, title, saved,
                captured_at, captured_epoch, saved_at, source_path, password_hash,
                base_png, scale_factor, monitor_json,
                next_id, annotations, anno_undo, anno_redo
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        )?;
        for (ord, tab) in tabs.iter().enumerate() {
            let doc = &tab.doc;
            stmt.execute(params![
                ord as i64,
                doc.meta.id,
                tab.title,
                tab.saved,
                doc.meta.captured_at.to_rfc3339(),
                doc.meta.captured_at.timestamp(),
                doc.meta.saved_at.as_ref().map(|d| d.to_rfc3339()),
                doc.meta
                    .source_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                doc.meta.password_hash,
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
        let captured_at = chrono::DateTime::parse_from_rfc3339(&self.captured_at)
            .map(|d| d.with_timezone(&chrono::Local))?;
        let saved_at = self
            .saved_at
            .map(|s| {
                chrono::DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&chrono::Local))
            })
            .transpose()?;
        let mut doc = Document::new(crate::capture::Capture {
            image: ser::png_decode(&self.base_png)?,
            scale_factor: self.scale_factor,
            monitor: serde_json::from_str(&self.monitor_json)?,
        });
        doc.meta = CaptureMeta {
            id: self.doc_id,
            captured_at,
            saved_at,
            source_path: self.source_path.map(PathBuf::from),
            password_hash: self.password_hash,
        };
        doc.annotations = postcard::from_bytes(&self.annotations)?;
        doc.set_next_id(self.next_id);
        doc.revision += 1;
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
                password_hash, base_png, scale_factor, monitor_json,
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
                base_png: row.get(7)?,
                scale_factor: row.get(8)?,
                monitor_json: row.get(9)?,
                next_id: row.get::<_, i64>(10)? as u64,
                annotations: row.get(11)?,
                anno_undo: row.get(12)?,
                anno_redo: row.get(13)?,
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
            image: image::RgbaImage::from_pixel(w, w, image::Rgba(color)),
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
                opacity: 0.5,
                shadow: 3.0,
            }),
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
    const EXPECTED: &str = "02010002040304ff0000ff0000004001010000ffff000080400000003f0000404002090000f04000000841030001020a141effffffffff00000040ffffffff0000403f0000c03f000200040002040304ff0000ff0000004001010000ffff000080400000003f000040400202050002040304ff0000ff0000004001010000ffff000080400000003f00004040060002040304ff0000ff0000004001010000ffff000080400000003f000040400101070002040304ff0000ff0000004001010000ffff000080400000003f0000404000";
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
}
