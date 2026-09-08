//! The async facade both hosts drive: a [`ListenTracker`] plus a
//! [`ListenStore`], every store call inside `spawn_blocking`.
//!
//! Hosts call explicit lifecycle verbs (`track_started`, `tick`,
//! `ended_naturally`, `stopped`, `handoff`, `shutdown`) and never touch SQLite
//! or the state machine themselves. "Listening history OFF" (`paused`) is
//! honoured HERE, in one place: a paused logger opens no row, so nothing
//! downstream has to ask.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use super::store::ListenStore;
use super::tracker::{ListenMeta, ListenTracker};

/// Where this process's rows come from (`origin_id`).
pub enum Origin {
    /// The desktop app: a uuid generated once per install and kept in
    /// `listen_meta`.
    Install,
    /// The daemon: `qbzd:<hostname>`, fixed.
    Daemon { hostname: String },
}

pub struct ListenLogger {
    store: Arc<Mutex<ListenStore>>,
    tracker: Mutex<ListenTracker>,
    app_session_id: String,
    origin_id: String,
    /// Mirror of `listen_meta.paused`, so the hot path (a tick per second)
    /// never opens the database to ask.
    paused: std::sync::atomic::AtomicBool,
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl ListenLogger {
    /// Open the per-user store, close any rows a previous run left open
    /// (`shutdown`), and mint this run's `app_session_id`.
    pub async fn open(user_dir: std::path::PathBuf, origin: Origin) -> Result<Arc<Self>, String> {
        let (store, origin_id, paused, orphans) = tokio::task::spawn_blocking(move || {
            let store = ListenStore::open_at(&user_dir)?;
            let origin_id = match origin {
                Origin::Install => store.origin_id_or_init(|| uuid::Uuid::new_v4().to_string())?,
                Origin::Daemon { hostname } => format!("qbzd:{hostname}"),
            };
            let paused = store.is_paused()?;
            let orphans = store.close_orphans_as_shutdown()?;
            Ok::<_, String>((store, origin_id, paused, orphans))
        })
        .await
        .map_err(|e| format!("listen log open task failed: {e}"))??;
        if orphans > 0 {
            log::info!(
                "[listen-log] closed {orphans} row(s) left open by a previous run as shutdown"
            );
        }
        log::info!("[listen-log] open (origin {origin_id}, paused={paused})");
        Ok(Arc::new(Self {
            store: Arc::new(Mutex::new(store)),
            tracker: Mutex::new(ListenTracker::new()),
            app_session_id: uuid::Uuid::new_v4().to_string(),
            origin_id,
            paused: std::sync::atomic::AtomicBool::new(paused),
        }))
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The "Listening history" toggle. Turning it OFF closes the row in
    /// flight (as `stop`) so nothing is written after the user said no.
    pub async fn set_paused(&self, paused: bool) -> Result<(), String> {
        self.paused
            .store(paused, std::sync::atomic::Ordering::Relaxed);
        if paused {
            let closed = self.tracker.lock().unwrap().stopped(now_unix(), false);
            if let Some(c) = closed {
                self.with_store(move |s| s.close_event(&c)).await?;
            }
        }
        self.with_store(move |s| s.set_paused(paused)).await
    }

    /// "Clear listening history": DELETE + VACUUM. The row in flight is
    /// closed first so it does not survive the clear.
    pub async fn clear(&self) -> Result<(), String> {
        let closed = self.tracker.lock().unwrap().stopped(now_unix(), false);
        self.with_store(move |s| {
            if let Some(c) = closed {
                s.close_event(&c)?;
            }
            s.clear()
        })
        .await
    }

    pub async fn count(&self) -> Result<u64, String> {
        self.with_store(|s| s.count()).await
    }

    /// A new track started (the host's de-duped track edge). Closes the row
    /// in flight as skip (or, with `infer_end`, as natural when it sat within
    /// 2 s of its end — for hosts without an explicit end edge).
    pub async fn track_started(&self, meta: ListenMeta, infer_end: bool) {
        let now = now_unix();
        if self.is_paused() {
            // Still close whatever was open before the pause landed.
            let closed = self.tracker.lock().unwrap().stopped(now, infer_end);
            if let Some(c) = closed {
                let _ = self.with_store(move |s| s.close_event(&c)).await;
            }
            return;
        }
        let (closed, meta) = {
            let mut t = self.tracker.lock().unwrap();
            if infer_end {
                t.track_started_inferring_end(meta, now)
            } else {
                t.track_started(meta, now)
            }
        };
        let session = self.app_session_id.clone();
        let origin = self.origin_id.clone();
        let result = self
            .with_store(move |s| {
                if let Some(c) = closed {
                    s.close_event(&c)?;
                }
                s.open_event(&session, &origin, &meta, now)
            })
            .await;
        match result {
            Ok(id) => self.tracker.lock().unwrap().opened(id),
            Err(e) => log::warn!("[listen-log] open failed: {e}"),
        }
    }

    /// One playback observation (1 Hz). Flushes to disk every ~10 s of play.
    pub async fn tick(&self, position_ms: u64, playing: bool) {
        let flush = self.tracker.lock().unwrap().tick(position_ms, playing);
        if let Some(f) = flush {
            if let Err(e) = self.with_store(move |s| s.flush_progress(&f)).await {
                log::warn!("[listen-log] flush failed: {e}");
            }
        }
    }

    pub async fn ended_naturally(&self) {
        let closed = self.tracker.lock().unwrap().ended_naturally(now_unix());
        self.close(closed).await;
    }

    pub async fn stopped(&self, infer_natural: bool) {
        let closed = self
            .tracker
            .lock()
            .unwrap()
            .stopped(now_unix(), infer_natural);
        self.close(closed).await;
    }

    pub async fn errored(&self) {
        let closed = self.tracker.lock().unwrap().errored(now_unix());
        self.close(closed).await;
    }

    /// Playback authority moved away from this process. Close the owner row
    /// as `handoff`; delegated tracks are intentionally never opened here.
    /// Idempotent so repeated lifecycle observations cannot rewrite the row.
    pub async fn handoff(&self) {
        let closed = self.tracker.lock().unwrap().handoff(now_unix());
        self.close(closed).await;
    }

    /// Orderly exit: close the row in flight as `shutdown`. SYNCHRONOUS on
    /// purpose — the hosts call it after their event loop is gone.
    pub fn shutdown_blocking(&self) {
        let closed = self.tracker.lock().unwrap().shutdown(now_unix());
        if let Some(c) = closed {
            if let Ok(store) = self.store.lock() {
                if let Err(e) = store.close_event(&c) {
                    log::warn!("[listen-log] shutdown close failed: {e}");
                }
            }
        }
    }

    /// Whether a row is in flight (tests + diagnostics).
    pub fn has_open_row(&self) -> bool {
        self.tracker.lock().unwrap().open().is_some()
    }

    /// Developer inspection: every row in id order.
    pub async fn rows(&self) -> Result<Vec<super::store::ListenRow>, String> {
        self.with_store(|s| s.rows()).await
    }

    async fn close(&self, closed: Option<super::tracker::Closed>) {
        if let Some(c) = closed {
            if let Err(e) = self.with_store(move |s| s.close_event(&c)).await {
                log::warn!("[listen-log] close failed: {e}");
            }
        }
    }

    async fn with_store<T, F>(&self, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&ListenStore) -> Result<T, String> + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let guard = store
                .lock()
                .map_err(|_| "listen log store lock poisoned".to_string())?;
            f(&guard)
        })
        .await
        .map_err(|e| format!("listen log task failed: {e}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::super::rules::EndReason;
    use super::*;

    fn meta(id: &str, duration_ms: u64) -> ListenMeta {
        ListenMeta {
            source: "qobuz".into(),
            source_item_id: id.into(),
            title: format!("T{id}"),
            artist: "A".into(),
            duration_ms,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn play_through_then_skip_then_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        assert_eq!(l.origin_id.len(), 36);

        l.track_started(meta("1", 30_000), false).await;
        for s in 0..=29u64 {
            l.tick(s * 1_000, true).await;
        }
        l.ended_naturally().await;

        l.track_started(meta("2", 200_000), false).await;
        for s in 0..=10u64 {
            l.tick(s * 1_000, true).await;
        }
        l.track_started(meta("3", 200_000), false).await;
        for s in 0..=25u64 {
            l.tick(s * 1_000, true).await;
        }
        l.shutdown_blocking();

        let rows = l.rows().await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].end_reason, Some(EndReason::Natural));
        assert_eq!(rows[0].played_ms, 29_000);
        assert_eq!(rows[1].end_reason, Some(EndReason::Skip));
        assert_eq!(rows[1].played_ms, 10_000);
        assert_eq!(rows[2].end_reason, Some(EndReason::Shutdown));
        assert_eq!(rows[2].played_ms, 25_000);
        assert!(rows.iter().all(|r| r.app_session_id == l.app_session_id));
    }

    #[tokio::test]
    async fn crash_leaves_flushed_progress_and_reopen_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let origin = {
            let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
                .await
                .unwrap();
            l.track_started(meta("1", 200_000), false).await;
            for s in 0..=23u64 {
                l.tick(s * 1_000, true).await;
            }
            // No shutdown: the process died here. The last flush was at 20 s.
            l.origin_id.clone()
        };
        let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        assert_eq!(l.origin_id, origin, "origin id survives restarts");
        let rows = l.rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].end_reason, Some(EndReason::Shutdown));
        assert_eq!(rows[0].played_ms, 20_000);
    }

    #[tokio::test]
    async fn paused_writes_nothing_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        l.set_paused(true).await.unwrap();
        l.track_started(meta("1", 100_000), false).await;
        l.tick(0, true).await;
        l.tick(15_000, true).await;
        l.ended_naturally().await;
        assert_eq!(l.count().await.unwrap(), 0);
        assert!(!l.has_open_row());

        // The flag is persisted: a reopen stays paused.
        let l2 = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        assert!(l2.is_paused());
        l2.set_paused(false).await.unwrap();
        l2.track_started(meta("2", 100_000), false).await;
        l2.tick(0, true).await;
        l2.tick(1_000, true).await;
        l2.stopped(false).await;
        let rows = l2.rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].end_reason, Some(EndReason::Stop));
        assert_eq!(rows[0].played_ms, 1_000);
    }

    #[tokio::test]
    async fn pausing_mid_track_closes_the_open_row_and_stops_writing() {
        let dir = tempfile::tempdir().unwrap();
        let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        l.track_started(meta("1", 100_000), false).await;
        l.tick(0, true).await;
        l.tick(1_000, true).await;
        l.set_paused(true).await.unwrap();
        l.tick(2_000, true).await;
        l.track_started(meta("2", 100_000), false).await;
        let rows = l.rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].end_reason, Some(EndReason::Stop));
        assert_eq!(rows[0].played_ms, 1_000);
    }

    #[tokio::test]
    async fn clear_removes_everything_including_the_open_row() {
        let dir = tempfile::tempdir().unwrap();
        let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        l.track_started(meta("1", 100_000), false).await;
        l.ended_naturally().await;
        l.track_started(meta("2", 100_000), false).await;
        l.clear().await.unwrap();
        assert_eq!(l.count().await.unwrap(), 0);
        assert!(!l.has_open_row());
        // Logging continues after a clear.
        l.track_started(meta("3", 100_000), false).await;
        assert_eq!(l.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn daemon_origin_and_inferred_end() {
        let dir = tempfile::tempdir().unwrap();
        let l = ListenLogger::open(
            dir.path().to_path_buf(),
            Origin::Daemon {
                hostname: "pi".into(),
            },
        )
        .await
        .unwrap();
        l.track_started(meta("1", 100_000), true).await;
        l.tick(0, true).await;
        l.tick(99_000, true).await;
        // The bus only says "another track started": infer natural.
        l.track_started(meta("2", 100_000), true).await;
        l.tick(0, true).await;
        l.tick(5_000, true).await;
        l.stopped(true).await;
        let rows = l.rows().await.unwrap();
        assert_eq!(rows[0].origin_id, "qbzd:pi");
        assert_eq!(rows[0].end_reason, Some(EndReason::Natural));
        assert_eq!(rows[1].end_reason, Some(EndReason::Stop));
    }

    #[tokio::test]
    async fn handoff_closes_the_owner_row_once() {
        let dir = tempfile::tempdir().unwrap();
        let l = ListenLogger::open(dir.path().to_path_buf(), Origin::Install)
            .await
            .unwrap();
        l.track_started(meta("1", 100_000), false).await;
        l.tick(0, true).await;
        l.tick(1_000, true).await;

        l.handoff().await;
        l.handoff().await;

        assert!(!l.has_open_row());
        let rows = l.rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].end_reason, Some(EndReason::Handoff));
        assert_eq!(rows[0].played_ms, 1_000);
    }
}
