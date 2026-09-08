//! One handle per source, instead of one handle per CALL.
//!
//! Today (bug 1): `LibraryDatabase::open(&path)` runs on **every** call, in
//! **two** accessors (`local_state.rs:39-61`, open at `:47`; `library_db_qt.rs:
//! 50-76`, open at `:62`), across 48 call sites in 17 files. Opening one MyQBZ
//! collection logged 23–34 `Opening library database` lines — one per item,
//! because collection expansion resolves every item independently. Before the
//! registry adapter, that drove one fresh local resolver open per item. A
//! 200-item collection therefore meant 200 opens.
//!
//! `open()` is not free: it runs the migrations and the pragmas.
//!
//! # Why there is no guard type
//!
//! `LibraryDatabase` wraps a `rusqlite::Connection`
//! (`qbz-library/src/database.rs:32-34`), which is `Send` but `!Sync`. So
//! `&LibraryDatabase` is `!Send` and must NEVER cross an `.await`. This type
//! enforces that BY API: it hands out no guard, only [`DbPool::with`]. There is no
//! `checkout() -> Guard` anywhere, public or crate-private. A future author
//! cannot write the bug.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use qbz_library::{LibraryDatabase, LibraryError};

/// How many idle handles are RETAINED.
///
/// It is NOT a concurrency cap: [`DbPool::with`] opens a fresh handle whenever
/// the free list is empty, so N truly concurrent callers still get N handles —
/// which is what makes the change no-worse than today in every case. Four is
/// what a burst leaves warm.
const POOL: usize = 4;

/// The per-user `library.db` handle set.
pub(crate) struct DbPool {
    /// The bound user's `library.db`. `None` = never bound this session (see
    /// the unbound fallback), or torn down.
    path: Mutex<Option<PathBuf>>,
    /// Free list of live handles, capped at [`POOL`]. `(generation, handle)`.
    idle: Mutex<Vec<(u64, LibraryDatabase)>>,
    /// Bumped on every rebind/teardown. A handle whose generation is stale is
    /// DROPPED on check-in and never re-served — the anti-leak that makes it
    /// impossible for user A's connection to answer user B's query. Correctness
    /// does not depend on anybody remembering to call `teardown`.
    generation: AtomicU64,
    /// Path computation for the UNBOUND window only.
    ///
    /// Today `db_path()` resolves the path per call from
    /// `UserDataPaths::load_last_user_id()` (local_state.rs:28-37,
    /// library_db_qt.rs:24-33), so `with_db` works at *any* moment, including
    /// before the auth path has bound anything. A pool that only knows the
    /// directory `bind_user` gave it would return `None` for every one of those
    /// calls — a silent regression, not a neutral change.
    ///
    /// It is INJECTED rather than computed here because `UserDataPaths` lives
    /// in `qbz-app`, and depending on `qbz-app` would drag
    /// `qbz-core`/`qbz-player`/`qbz-audio` into this crate's graph (design §8
    /// forbids it — the PROTECTED backend must not be linkable from here).
    ///
    /// The fallback exists so a HALF-APPLIED stage 1 degrades to today's
    /// behaviour instead of to "no local library". It is removed once every
    /// entry point provably runs after `bind_user`.
    fallback: Mutex<Option<Box<dyn Fn() -> Option<PathBuf> + Send + Sync>>>,
    /// One `warn!` per process for the unbound window, not one per call.
    warned_unbound: AtomicBool,
}

impl DbPool {
    pub(crate) fn new() -> Self {
        Self {
            path: Mutex::new(None),
            idle: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
            fallback: Mutex::new(None),
            warned_unbound: AtomicBool::new(false),
        }
    }

    /// Install the unbound-window path computation (see [`DbPool::fallback`]).
    pub(crate) fn set_unbound_fallback(
        &self,
        f: Option<Box<dyn Fn() -> Option<PathBuf> + Send + Sync>>,
    ) {
        if let Ok(mut slot) = self.fallback.lock() {
            *slot = f;
        }
    }

    /// Bind (or re-bind) to a user directory. Bumps the generation and drains
    /// the free list, so no outstanding handle can serve the new user.
    pub(crate) fn bind(&self, dir: &Path) {
        if let Ok(mut p) = self.path.lock() {
            *p = Some(dir.join("library.db"));
        }
        self.invalidate();
    }

    /// Drop every handle and forget the binding.
    ///
    /// NOTE this is a behaviour FIX, not a neutral change: today a post-logout
    /// `with_db` still resolves through `load_last_user_id()` and can return
    /// the PREVIOUS account's rows. After teardown the pool refuses reads, so a
    /// view that used to render stale data after logout now renders empty.
    pub(crate) fn teardown(&self) {
        if let Ok(mut p) = self.path.lock() {
            *p = None;
        }
        self.invalidate();
    }

    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut idle) = self.idle.lock() {
            idle.clear();
        }
    }

    /// READ semantics: `None` when there is no session or the file does not
    /// exist. Never creates.
    ///
    /// Moved from `local_state::with_db` (local_state.rs:39-61) — the SIGNATURE
    /// of that function does not change, only its body, so its 48 call sites
    /// are untouched.
    pub(crate) fn with<R>(
        &self,
        f: impl FnOnce(&LibraryDatabase) -> Result<R, LibraryError>,
    ) -> Option<R> {
        self.run(false, f)
    }

    /// WRITE semantics: creates the directory and the file when absent.
    ///
    /// Moved from `library_db_qt::with_db(create = true)`
    /// (library_db_qt.rs:50-76). Both semantics are kept deliberately:
    /// `library_db_qt.rs:41-49` records why — a brand-new account has no
    /// `library.db`, the read-only accessor returns `None`, and through it the
    /// mixtape migrations could never run, so the user could never create a
    /// collection. Collapsing the two accessors would resurrect that.
    pub(crate) fn with_creating<R>(
        &self,
        f: impl FnOnce(&LibraryDatabase) -> Result<R, LibraryError>,
    ) -> Option<R> {
        self.run(true, f)
    }

    /// Check-out → run `f` with NO lock held → check-in.
    ///
    /// Holding `idle` across `f` would (a) serialise every reader and (b)
    /// **self-deadlock** on any re-entrant `with_db(|db| … with_db(…) …)` — a
    /// shape today's open-per-call tolerates for free, so a design that breaks
    /// it is not behaviour-neutral. There is no `R: Send` bound: nothing here
    /// crosses a thread.
    fn run<R>(
        &self,
        create: bool,
        f: impl FnOnce(&LibraryDatabase) -> Result<R, LibraryError>,
    ) -> Option<R> {
        let (generation, db) = self.checkout(create)?;
        let out = match f(&db) {
            Ok(r) => Some(r),
            Err(e) => {
                // NOTE: the two accessors this replaces logged DIFFERENT
                // strings (local_state.rs:49/57 "local library …" vs
                // library_db_qt.rs:65/72 "library.db …"). One pool emits one
                // set; anything grepping the old text must be updated once.
                log::error!("[qbz-source] library.db op failed: {e}");
                None
            }
        };
        self.checkin(generation, db);
        out
    }

    /// Pop a live handle, or open a fresh one. The `idle` lock is released
    /// before this returns.
    fn checkout(&self, create: bool) -> Option<(u64, LibraryDatabase)> {
        let generation = self.generation.load(Ordering::Acquire);
        if let Ok(mut idle) = self.idle.lock() {
            while let Some((g, db)) = idle.pop() {
                if g == generation {
                    return Some((g, db));
                }
                // Stale generation: drop it here rather than serve it.
                drop(db);
            }
        }
        let path = self.resolve_path()?;
        if create {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        } else if !path.exists() {
            return None;
        }
        match LibraryDatabase::open(&path) {
            Ok(db) => Some((generation, db)),
            Err(e) => {
                log::warn!("[qbz-source] library.db open failed ({}): {e}", path.display());
                None
            }
        }
    }

    /// Return the handle, dropping it if it is stale or the free list is full.
    fn checkin(&self, generation: u64, db: LibraryDatabase) {
        if generation != self.generation.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut idle) = self.idle.lock() {
            if idle.len() < POOL {
                idle.push((generation, db));
            }
        }
    }

    fn resolve_path(&self) -> Option<PathBuf> {
        if let Ok(p) = self.path.lock() {
            if let Some(path) = p.as_ref() {
                return Some(path.clone());
            }
        }
        // Unbound window only: never bound this session. After a teardown the
        // generation is non-zero and the pool refuses reads (see `teardown`).
        if self.generation.load(Ordering::Acquire) != 0 {
            return None;
        }
        let path = {
            let slot = self.fallback.lock().ok()?;
            (slot.as_ref()?)()
        };
        if path.is_some() && !self.warned_unbound.swap(true, Ordering::AcqRel) {
            log::warn!(
                "[qbz-source] library.db accessed before bind_user; using the unbound fallback path"
            );
        }
        path
    }
}

impl Default for DbPool {
    fn default() -> Self {
        Self::new()
    }
}
