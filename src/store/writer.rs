//! The background session writer: a dedicated thread owns the [`Store`]
//! for the whole run and drains an ordered job queue, so PNG encoding
//! and SQLite I/O never touch the UI thread.
//!
//! Semantics:
//! - **Ordered**: jobs run strictly FIFO — a capture create, a later
//!   edit of the same document and a tab close always land in the order
//!   the user produced them.
//! - **Loss window**: every [`DocWrite`] carries the instant it was
//!   queued; the worker logs `queued_ms` (enqueue → committed), which
//!   IS the window of data a crash would lose.
//! - **Failures never block**: a failing job is retried once in place,
//!   then parked and retried again at the next flush; the affected
//!   document reports a [`WriteEvent::DocFailed`] so its tab can flag
//!   the problem instead of losing data silently.
//! - **Flush at exit**: `flush()` replies only once the queue is empty
//!   AND parked jobs have been retried — the quit path bounds the wait
//!   itself (timeout + warn).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use super::{DocWrite, MetaRow, Store, StoredSession};

/// One unit of work for the writer thread.
enum Job {
    /// Upsert one document row (PNG encode iff `base` is `Some`).
    Doc(Box<DocWrite>),
    /// Refresh the meta columns of every listed row.
    Meta(Vec<MetaRow>),
    /// Write the session row (active tab, title counter).
    Session {
        /// Active tab index.
        active: usize,
        /// Auto-title counter.
        next_title: usize,
    },
    /// Delete the rows of closed tabs.
    Delete(Vec<String>),
    /// Hard-purge every row of a deleted workspace (safety net —
    /// viewer-side deletion only happens on EMPTY workspaces).
    DeleteWorkspace(String),
    /// Empty the store (session discard).
    Clear,
    /// Apply the retention policy (startup + settings change).
    Retention(crate::config::DbRetention),
    /// Load the persisted session — replied over the channel.
    Load(mpsc::Sender<StoredSession>),
    /// Quit-time polish: meta refresh + stale-row prune.
    Finalize {
        /// Every live document's meta row.
        rows: Vec<MetaRow>,
        /// Ids of documents that may stay.
        live: Vec<String>,
        /// Active tab index.
        active: usize,
        /// Auto-title counter.
        next_title: usize,
    },
    /// Measured VACUUM — queued between Finalize and Flush at quit so
    /// the compaction runs on this thread, after all writes, outside
    /// any transaction.
    Vacuum,
    /// Barrier: reply when the whole queue is drained and parked jobs
    /// have been retried. The quit path blocks on this.
    Flush(mpsc::Sender<()>),
    /// Last job ever: the sender side is gone, exit the loop cleanly so
    /// the `Store` drops (VACUUM) on this thread.
    Shutdown,
}

/// Something the UI should know about — drained once per pump.
pub enum WriteEvent {
    /// A document row was durably written.
    DocWritten {
        /// The document id.
        id: String,
        /// Enqueue → committed, ms — the loss window for this write.
        queued_ms: u64,
        /// Encode + SQLite work, ms.
        work_ms: u64,
    },
    /// A document write failed even after the in-place retry — it is
    /// parked for the flush retry; the tab must show the warning glyph.
    DocFailed {
        /// The document id.
        id: String,
    },
}

/// The UI-side handle: enqueue jobs, drain events, read the row count.
/// `Drop` sends `Shutdown` and joins the thread so the final `Store`
/// drop (VACUUM) still runs — even on an exit path that skipped the
/// flush (e.g. a panic unwind).
pub struct Writer {
    tx: mpsc::Sender<Job>,
    /// `mpsc::Receiver` is not `Sync` — the mutex lets `Writer` live
    /// inside the shared `Arc`.
    event_rx: Mutex<mpsc::Receiver<WriteEvent>>,
    /// Rows currently in the database — kept fresh by the worker so
    /// `document_count` never waits behind a PNG encode.
    count: Arc<AtomicUsize>,
    /// Write jobs queued or executing — the status strip's WRITE
    /// indicator: >0 while any Doc/Meta/Session/Delete/DeleteWorkspace/
    /// Clear/Retention/Finalize job is in flight.
    pending_writes: Arc<AtomicUsize>,
    /// `true` while a `Load` job runs — the READ indicator.
    reading: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Writer {
    /// Spawns the `pixme-store` thread around `store`. `wake` is called
    /// after every event so a sleeping UI notices a failure promptly —
    /// `None` in tests.
    pub fn spawn(store: Store, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let (event_tx, event_rx) = mpsc::channel::<WriteEvent>();
        let count = Arc::new(AtomicUsize::new(store.document_count()));
        let worker_count = Arc::clone(&count);
        let pending_writes = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending_writes);
        let reading = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_reading = Arc::clone(&reading);
        let handle = std::thread::Builder::new()
            .name("pixme-store".to_owned())
            .spawn(move || {
                run(
                    store,
                    rx,
                    event_tx,
                    worker_count,
                    worker_pending,
                    worker_reading,
                    wake,
                )
            })
            .expect("spawn pixme-store");
        Self {
            tx,
            event_rx: Mutex::new(event_rx),
            count,
            pending_writes,
            reading,
            handle: Some(handle),
        }
    }

    /// Enqueue a job; a dead worker (it cannot — the channel outlives
    /// the handle) only logs.
    fn send(&self, job: Job) {
        if matches!(
            job,
            Job::Doc(_)
                | Job::Meta(_)
                | Job::Session { .. }
                | Job::Delete(_)
                | Job::DeleteWorkspace(_)
                | Job::Clear
                | Job::Retention(_)
                | Job::Finalize { .. }
        ) {
            self.pending_writes.fetch_add(1, Ordering::Relaxed);
        }
        if self.tx.send(job).is_err() {
            tracing::warn!("store writer gone — job dropped");
        }
    }

    /// Upsert one document row.
    pub fn write_doc(&self, w: DocWrite) {
        self.send(Job::Doc(Box::new(w)));
    }

    /// Refresh the meta columns of every listed row.
    pub fn write_meta(&self, rows: Vec<MetaRow>) {
        self.send(Job::Meta(rows));
    }

    /// Write the session row.
    pub fn set_session(&self, active: usize, next_title: usize) {
        self.send(Job::Session { active, next_title });
    }

    /// Delete the rows of closed tabs.
    pub fn delete(&self, ids: Vec<String>) {
        self.send(Job::Delete(ids));
    }

    /// Purge every row of a deleted workspace.
    pub fn delete_workspace(&self, id: String) {
        self.send(Job::DeleteWorkspace(id));
    }

    /// Empty the store (session discard).
    pub fn clear(&self) {
        self.send(Job::Clear);
    }

    /// Apply the retention policy.
    pub fn retention(&self, retention: crate::config::DbRetention) {
        self.send(Job::Retention(retention));
    }

    /// Load the persisted session on the worker (PNG decodes stay off
    /// the UI thread); the session comes back over the receiver.
    pub fn load(&self) -> mpsc::Receiver<StoredSession> {
        let (tx, rx) = mpsc::channel();
        self.send(Job::Load(tx));
        rx
    }

    /// The quit-time polish, queued behind every eager write.
    pub fn finalize(
        &self,
        rows: Vec<MetaRow>,
        live: Vec<String>,
        active: usize,
        next_title: usize,
    ) {
        self.send(Job::Finalize {
            rows,
            live,
            active,
            next_title,
        });
    }

    /// The exit barrier: the receiver fires once the queue is empty
    /// and parked failures have been retried.
    /// Queues the quit-time measured VACUUM after the pending writes.
    pub fn vacuum(&self) {
        self.send(Job::Vacuum);
    }

    pub fn flush(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        self.send(Job::Flush(tx));
        rx
    }

    /// Rows currently in the database — the settings warning only
    /// needs "is there something to lose".
    pub fn document_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Write jobs queued or in flight — the status strip's WRITE
    /// indicator (>0 = activity).
    pub fn pending_writes(&self) -> usize {
        self.pending_writes.load(Ordering::Relaxed)
    }

    /// `true` while a `Load` runs — the status strip's READ indicator.
    pub fn is_reading(&self) -> bool {
        self.reading.load(Ordering::Relaxed)
    }

    /// Everything the worker reported since the last drain.
    pub fn drain_events(&self) -> Vec<WriteEvent> {
        let rx = self
            .event_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

impl Drop for Writer {
    /// `Shutdown` rides the same queue: every job queued before it
    /// still runs, then the thread exits and the `Store` drops (the
    /// pending VACUUM is its Drop-side work).
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(handle) = self.handle.take()
            && let Err(err) = handle.join()
        {
            tracing::warn!("store writer join failed: {err:?}");
        }
    }
}

/// The worker loop: FIFO over `rx`, one in-place retry on failure,
/// parked jobs retried at `Flush`. A job that panics is dropped like a
/// failure — the loop must outlive any single bad write.
fn run(
    mut store: Store,
    rx: mpsc::Receiver<Job>,
    event_tx: mpsc::Sender<WriteEvent>,
    count: Arc<AtomicUsize>,
    pending_writes: Arc<AtomicUsize>,
    reading: Arc<std::sync::atomic::AtomicBool>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    // Sends an event and wakes the UI so it lands promptly.
    let emit = |ev: WriteEvent| {
        let _ = event_tx.send(ev);
        if let Some(w) = &wake {
            w();
        }
    };

    /// Executes `job` once; returns `true` on success. Panics are
    /// contained — a poisoned job must not kill the writer.
    fn execute(store: &mut Store, job: &Job, emit: &dyn Fn(WriteEvent)) -> bool {
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute_inner(store, job, emit)
        }));
        match out {
            Ok(ok) => ok,
            Err(_) => {
                tracing::error!("store job panicked; dropped");
                false
            }
        }
    }

    fn execute_inner(store: &mut Store, job: &Job, emit: &dyn Fn(WriteEvent)) -> bool {
        match job {
            Job::Doc(w) => {
                let t0 = std::time::Instant::now();
                match store.write_doc(w) {
                    Ok(()) => {
                        let work_ms = t0.elapsed().as_millis() as u64;
                        let queued_ms = w.queued_at.elapsed().as_millis() as u64;
                        tracing::debug!(
                            id = %w.id,
                            queued_ms,
                            work_ms,
                            "session doc written"
                        );
                        emit(WriteEvent::DocWritten {
                            id: w.id.clone(),
                            queued_ms,
                            work_ms,
                        });
                        true
                    }
                    Err(err) => {
                        tracing::warn!(id = %w.id, "session write failed: {err:#}");
                        false
                    }
                }
            }
            Job::Vacuum => {
                store.vacuum_if_needed();
                true
            }
            Job::Meta(rows) => match store.write_meta(rows) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!("session meta write failed: {err:#}");
                    false
                }
            },
            Job::Session { active, next_title } => match store.set_session(*active, *next_title) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!("session row write failed: {err:#}");
                    false
                }
            },
            Job::Delete(ids) => {
                store.delete_documents(ids);
                true
            }
            Job::DeleteWorkspace(id) => {
                store.delete_workspace(id);
                true
            }
            Job::Clear => {
                store.clear();
                true
            }
            Job::Retention(r) => {
                store.apply_retention(*r);
                true
            }
            Job::Load(reply) => {
                let _ = reply.send(store.load_session());
                true
            }
            Job::Finalize {
                rows,
                live,
                active,
                next_title,
            } => match store.finalize(rows, live, *active, *next_title) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!("session finalize failed: {err:#}");
                    false
                }
            },
            // Handled by the caller (`Flush` replies, `Shutdown` breaks).
            Job::Flush(_) | Job::Shutdown => true,
        }
    }

    /// `true` when the job mutates the document table — the count is
    /// refreshed afterwards.
    fn mutates(job: &Job) -> bool {
        matches!(
            job,
            Job::Doc(_)
                | Job::Delete(_)
                | Job::DeleteWorkspace(_)
                | Job::Clear
                | Job::Retention(_)
                | Job::Finalize { .. }
        )
    }

    /// `true` for jobs counted in the WRITE activity indicator — the
    /// same set `send` increments on.
    fn is_write(job: &Job) -> bool {
        matches!(
            job,
            Job::Doc(_)
                | Job::Meta(_)
                | Job::Session { .. }
                | Job::Delete(_)
                | Job::DeleteWorkspace(_)
                | Job::Clear
                | Job::Retention(_)
                | Job::Finalize { .. }
        )
    }

    /// Runs `job`, retrying once in place on failure. Doc jobs that
    /// still fail emit `DocFailed` and are returned for the flush
    /// retry; everything else is dropped after logging.
    fn run_one(
        store: &mut Store,
        job: Job,
        emit: &dyn Fn(WriteEvent),
        count: &AtomicUsize,
        emit_doc_failed: bool,
    ) -> Option<Job> {
        let refresh = mutates(&job);
        if execute(store, &job, emit) || execute(store, &job, emit) {
            if refresh {
                count.store(store.document_count(), Ordering::Relaxed);
            }
            return None;
        }
        match job {
            Job::Doc(w) => {
                if emit_doc_failed {
                    emit(WriteEvent::DocFailed { id: w.id.clone() });
                }
                Some(Job::Doc(w))
            }
            // Non-document jobs only get the flush retry — no UI flag.
            other => Some(other),
        }
    }

    let mut parked: Vec<Job> = Vec::new();
    while let Ok(mut job) = rx.recv() {
        if let Job::Flush(reply) = job {
            // Everything queued before the flush is done: parked
            // failures get their last retry now, then the barrier lifts.
            for parked_job in std::mem::take(&mut parked) {
                let _ = run_one(&mut store, parked_job, &emit, &count, false);
            }
            let _ = reply.send(());
            if let Some(w) = &wake {
                w();
            }
            continue;
        }
        if matches!(job, Job::Shutdown) {
            break;
        }
        // Keep `parked` coherent with newer jobs: a fresh `Doc`
        // supersedes an older parked snapshot for the same id (inheriting
        // its `base` pixels if the initial insert was the one that
        // failed); a `Delete` or `Clear` drops parked writes so a flush
        // never resurrects a closed tab or a discarded session.
        if let Job::Doc(w) = &mut job {
            for p in &mut parked {
                if let Job::Doc(prev) = p
                    && prev.id == w.id
                    && w.base.is_none()
                {
                    w.base = prev.base.take();
                }
            }
            let id = w.id.clone();
            parked.retain(|p| !matches!(p, Job::Doc(prev) if prev.id == id));
        } else if let Job::Delete(ids) = &job {
            parked.retain(|p| !matches!(p, Job::Doc(prev) if ids.contains(&prev.id)));
        } else if let Job::DeleteWorkspace(id) = &job {
            parked.retain(|p| !matches!(p, Job::Doc(prev) if prev.workspace_id == *id));
        } else if matches!(
            job,
            Job::Clear | Job::Retention(crate::config::DbRetention::Never)
        ) {
            parked.clear();
        }
        // A finished load hands the session back — the viewer sleeps
        // until woken, same for every event the jobs emit inside.
        let is_load = matches!(job, Job::Load(_));
        let is_write_job = is_write(&job);
        if is_load {
            reading.store(true, Ordering::Relaxed);
        }
        if let Some(failed) = run_one(&mut store, job, &emit, &count, true) {
            parked.push(failed);
        }
        if is_load {
            reading.store(false, Ordering::Relaxed);
        }
        if is_write_job {
            pending_writes.fetch_sub(1, Ordering::Relaxed);
        }
        if (is_load || is_write_job)
            && let Some(w) = &wake
        {
            w();
        }
    }
    tracing::info!("store writer stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, MonitorInfo, PhysicalRect};
    use crate::editor::Document;
    use crate::ui::viewer::Viewer;

    /// A viewer holding `n` stub tabs — the DocWrite sources.
    fn viewer_with(n: usize) -> Viewer {
        let mut viewer = Viewer::default();
        for i in 0..n {
            let doc = Document::new(Capture {
                image: Arc::new(image::RgbaImage::from_pixel(4, 4, image::Rgba([1; 4]))),
                scale_factor: 1.0,
                monitor: MonitorInfo {
                    id: 0,
                    name: String::new(),
                    rect: PhysicalRect {
                        x: 0,
                        y: 0,
                        width: 4,
                        height: 4,
                    },
                    scale_factor: 1.0,
                    is_primary: true,
                },
            });
            viewer.push_named_document(doc, format!("tab {i}"), false);
        }
        viewer
    }

    /// Writes land in FIFO order and the flush barrier only lifts once
    /// the queue is empty — every enqueued doc committed, events seen.
    #[test]
    fn writes_commit_in_order_and_flush_reports() {
        let writer = Writer::spawn(Store::open_in_memory(), None);
        let viewer = viewer_with(3);
        for (ord, tab) in viewer.tabs.iter().enumerate() {
            writer.write_doc(tab.to_doc_write(ord));
        }
        writer.set_session(1, 3);
        writer.flush().recv().expect("flush reply");
        assert_eq!(writer.document_count(), 3);
        let written = writer
            .drain_events()
            .iter()
            .filter(|ev| matches!(ev, WriteEvent::DocWritten { .. }))
            .count();
        assert_eq!(written, 3);
    }

    /// A closed tab's delete job removes its row — and a following
    /// flush still reports an empty queue.
    #[test]
    fn delete_removes_the_row() {
        let writer = Writer::spawn(Store::open_in_memory(), None);
        let viewer = viewer_with(2);
        for (ord, tab) in viewer.tabs.iter().enumerate() {
            writer.write_doc(tab.to_doc_write(ord));
        }
        writer.delete(vec![viewer.tabs[0].doc.meta.id.clone()]);
        writer.flush().recv().expect("flush reply");
        assert_eq!(writer.document_count(), 1);
    }

    /// `Load` returns the stored session over the reply channel — the
    /// same path the viewer's restore takes.
    #[test]
    fn load_roundtrips_the_session() {
        let writer = Writer::spawn(Store::open_in_memory(), None);
        let viewer = viewer_with(2);
        for (ord, tab) in viewer.tabs.iter().enumerate() {
            writer.write_doc(tab.to_doc_write(ord));
        }
        writer.flush().recv().expect("flush reply");
        let session = writer.load().recv().expect("load reply");
        assert_eq!(session.tabs.len(), 2);
    }

    /// A failed write emits `DocFailed` and parks; a later full write
    /// for the same document supersedes the parked failure and commits.
    #[test]
    fn failed_write_reports_and_later_doc_supersedes_parked() {
        let writer = Writer::spawn(Store::open_in_memory(), None);
        let viewer = viewer_with(1);
        let id = viewer.tabs[0].doc.meta.id.clone();
        // `base = None` on a non-existent row fails the UPDATE twice
        // and parks the job.
        let mut bad = viewer.tabs[0].to_doc_write(0);
        bad.base = None;
        writer.write_doc(bad);
        // Queue a full write right behind it: it supersedes the parked
        // failure and inserts the row.
        writer.write_doc(viewer.tabs[0].to_doc_write(0));
        writer.flush().recv().expect("flush reply");
        assert_eq!(writer.document_count(), 1);
        let events = writer.drain_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WriteEvent::DocFailed { id: fid } if *fid == id))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WriteEvent::DocWritten { id: wid, .. } if *wid == id))
        );
    }
}
