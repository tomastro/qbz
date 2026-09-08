//! Playlist Importer — port of `crates/qbz/src/playlist_import.rs` (the
//! controller) plus the `PlaylistImportActions` handler arms in the
//! reference's `main.rs:21867-22045` (the async plumbing, the folder
//! assignment, the toast, the sidebar reconcile and the navigation), driven by
//! the headless `qbz-playlist-import` crate.
//!
//! Two-step flow: URL entry -> provider auto-detect -> fetch preview -> rename
//! + optional folder -> import with a live progress bar, a status line and an
//! append-only log, then an in-panel Summary block. Every interpolated string
//! (log lines, status line, summary block) is formatted HERE and lands in the
//! document pre-formatted; the modal renders it verbatim, exactly as the
//! reference does.
//!
//! # Close-mid-import semantics (reference §1.8) — ported verbatim
//!
//! Closing the modal NEVER cancels the tokio import task. On completion the
//! toast + folder assignment + sidebar refresh still fire; navigation happens
//! only while the modal is still open AND the run's generation is current.
//! [`GENERATION`] is bumped on every [`open`] and [`execute`], so a stale run's
//! sink events and completion arms can never touch a reopened modal's fresh
//! state.
//!
//! # Where this port deviates, and why (recorded, not silent)
//!
//! 1. **The folder dropdown reads the DB itself.** The reference seeds it from
//!    `SidebarState.folders` (`playlist_import.rs:88-101`). Doc 05 §5.8.2
//!    forbids that here: the sidebar must not become a data source for another
//!    domain, so [`open`] spawns a `folders_qt::load_folders_full()` read and
//!    republishes. Hidden folders are filtered out; index 0 is always
//!    `No folder` with id `""`, the create-playlist builder shape.
//!
//! 2. **Progress publishes are coalesced at ~60 ms; the STATE still updates on
//!    every event.** The reference sets four Slint properties per sink event
//!    and pushes one row into a `VecModel` — both O(1). Here a publish
//!    re-serialises the WHOLE document and QML re-parses it and rebuilds the
//!    log `Repeater`, and matching emits one event PER TRACK (thousands on a
//!    large playlist). So [`publish_progress`] rate-limits with a guaranteed
//!    trailing flush, while every log append, phase change and terminal arm
//!    publishes unconditionally. Nothing observable is dropped: a 6 px bar and
//!    a status line cannot show more than ~16 fps of change, and the final
//!    value always lands through the terminal publish.
//!
//! 3. **No cancel, still** — there is no cancel in the reference and none is
//!    invented here.
//!
//! Everything else — the two-step gate, the locked provider, the
//! rearm-on-URL-edit path after a completed import, the 5 %-milestone matching
//! log, the per-chunk adding log, the summary block, the `parts_line` reuse and
//! the fixed en-US thousands grouping — is 1:1.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use cxx_qt_lib::QString;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use serde::Serialize;

use qbz_playlist_import::sources::service::{lastfm, listenbrainz};
use qbz_playlist_import::{
    detect_provider_key, ImportEvent, ImportPhase, ImportPlaylist, ImportProgressSink,
    ImportProvider, ImportSummary, LastFmStation, PlaylistImportError, PlaylistSource, ProviderKey,
    MAX_IMPORT_BYTES,
};

/// The source picker's indices. They are the ONE place the picker order is
/// defined; the QML reads its labels from `sourceOptions` and sends back an
/// index, so a reorder here is a reorder there and nowhere else.
mod source_kind {
    pub const URL: i32 = 0;
    pub const FILE: i32 = 1;
    pub const JSON: i32 = 2;
    pub const LISTENBRAINZ: i32 = 3;
    pub const LASTFM: i32 = 4;
}

type Runtime = Arc<AppRuntime<LoggingAdapter>>;

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

/// One append-only line of the conversion log (`ImportLogEntry` in the
/// reference's `state.slint:5814`). `status` is `"info" | "success" | "error"`
/// and drives the row colour.
#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub message: String,
    pub status: String,
}

/// `QbzPlaylistImport.importJson`. ONE document for the whole modal — the
/// picker/manager convention.
#[derive(Default, Serialize)]
struct ImportDoc {
    open: bool,
    /// Bumped by every [`open`]. QML resets its two local text mirrors when it
    /// changes, which is what reproduces the reference's "Tauri remounts the
    /// component on every open" behaviour (§5.8.3).
    #[serde(rename = "resetSeq")]
    reset_seq: i64,
    /// Rust's mirror of the URL field — the re-seed source for the QML input
    /// while it does NOT have focus, and `""` right after a reset.
    url: String,
    /// Same, for the rename field. Written by Rust on a successful preview
    /// (prefilled with the source playlist name).
    #[serde(rename = "customName")]
    custom_name: String,
    /// True during the preview fetch AND during the import execute.
    loading: bool,
    /// `""` = none; the red banner at the top of the body.
    error: String,
    /// `"" | "spotify" | "apple" | "tidal" | "deezer"` — the locked-or-detected
    /// provider; its source logo renders at full opacity.
    #[serde(rename = "activeProvider")]
    active_provider: String,
    /// Provider detected AND not offline.
    #[serde(rename = "canFetch")]
    can_fetch: bool,
    /// A preview exists AND the URL has not been edited since the fetch —
    /// flips the modal into step B (rename + Import).
    #[serde(rename = "showPreview")]
    show_preview: bool,
    #[serde(rename = "importCompleted")]
    import_completed: bool,
    /// The "Conversion progress" panel is mounted.
    #[serde(rename = "progressVisible")]
    progress_visible: bool,
    /// The bar + the status/current-track lines are visible.
    #[serde(rename = "hasProgress")]
    has_progress: bool,
    /// 0..1.
    progress: f32,
    #[serde(rename = "statusLine")]
    status_line: String,
    #[serde(rename = "currentTrack")]
    current_track: String,
    log: Vec<LogEntry>,
    // --- Source picker (2.0.3 expansion) --------------------------------
    /// 0 URL · 1 Playlist file · 2 JSON · 3 ListenBrainz · 4 Last.fm.
    #[serde(rename = "sourceIndex")]
    source_index: i32,
    /// The five localized picker labels, formatted here like every other
    /// string in this document.
    #[serde(rename = "sourceOptions")]
    source_options: Vec<String>,
    /// `""` = nothing picked yet.
    #[serde(rename = "pickedFileName")]
    picked_file_name: String,
    /// The ListenBrainz / Last.fm username-or-URL field.
    #[serde(rename = "serviceUser")]
    service_user: String,
    /// Last.fm only: 0 = a profile (show the station picker), 1 = a specific
    /// playlist (import it directly, no station choice).
    #[serde(rename = "lastfmMode")]
    lastfm_mode: i32,
    #[serde(rename = "stationOptions")]
    station_options: Vec<String>,
    #[serde(rename = "stationIndex")]
    station_index: i32,
    /// ListenBrainz "created for you" titles, filled after a valid username.
    #[serde(rename = "lbPlaylistOptions")]
    lb_playlist_options: Vec<String>,
    #[serde(rename = "lbPlaylistIndex")]
    lb_playlist_index: i32,
    /// True while the "created for you" list is being fetched.
    #[serde(rename = "lbListLoading")]
    lb_list_loading: bool,

    /// Index 0 is always `No folder` (id `""`), then the visible folders.
    #[serde(rename = "folderOptions")]
    folder_options: Vec<String>,
    #[serde(rename = "folderIds")]
    folder_ids: Vec<String>,
    #[serde(rename = "folderIndex")]
    folder_index: i32,
    /// Summary block, pre-formatted lines (`""` = hidden).
    #[serde(rename = "summaryPlaylist")]
    summary_playlist: String,
    #[serde(rename = "summaryMatched")]
    summary_matched: String,
    #[serde(rename = "summarySkipped")]
    summary_skipped: String,
    /// `""` unless the source repeated a track. Its own line rather than a
    /// footnote on "skipped": a duplicate is not a failure and reading it as
    /// one is what made a 453-of-469 match look like a 198-of-469 one.
    #[serde(rename = "summaryDuplicates")]
    summary_duplicates: String,
    #[serde(rename = "summaryParts")]
    summary_parts: String,
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Everything the modal owns, published fields and session-only fields alike.
/// Reset wholesale on every [`open`] — the reference clears 14 Slint
/// properties plus its `Session` struct for the same reason
/// (`playlist_import.rs:66-104`).
#[derive(Default)]
struct State {
    // ---- published ----
    open: bool,
    url: String,
    custom_name: String,
    loading: bool,
    error: String,
    active_provider: String,
    can_fetch: bool,
    show_preview: bool,
    import_completed: bool,
    progress_visible: bool,
    has_progress: bool,
    progress: f32,
    status_line: String,
    current_track: String,
    log: Vec<LogEntry>,
    source_index: i32,
    source_options: Vec<String>,
    picked_file_name: String,
    service_user: String,
    lastfm_mode: i32,
    station_options: Vec<String>,
    station_index: i32,
    lb_playlist_options: Vec<String>,
    lb_playlist_index: i32,
    lb_list_loading: bool,
    folder_options: Vec<String>,
    folder_ids: Vec<String>,
    folder_index: i32,
    summary_playlist: String,
    summary_matched: String,
    summary_skipped: String,
    summary_duplicates: String,
    summary_parts: String,

    // ---- session-only (the reference's `Session`) ----
    /// The fetched preview, kept so `execute` can read the source name.
    preview: Option<ImportPlaylist>,
    /// Trimmed URL the preview was fetched for (Svelte `previewUrl`).
    preview_url: String,
    /// Provider locked at fetch time; survives URL edits until a reset path
    /// clears it (Svelte `lockedProvider`).
    locked_provider: Option<ProviderKey>,
    /// Trimmed URL of the last completed import (Svelte `lastImportedUrl`).
    last_imported_url: String,
    /// 5 %-milestone tracker for the matching log lines (-1 = none yet).
    last_logged_percent: i32,

    // ---- source-specific session state (2.0.3) ----
    /// The picked file's bytes. They live HERE, not on disk and not re-read at
    /// execute time: the `rfd` handle is gone by then and the dialog cannot
    /// silently reopen. Bounded by `MAX_IMPORT_BYTES` before the read.
    file_bytes: Option<Vec<u8>>,
    file_name: String,
    /// The MBIDs parallel to `lb_playlist_options`.
    lb_playlist_mbids: Vec<String>,
}

impl State {
    /// The reset every [`open`] performs, folder list included — the folders
    /// are re-read asynchronously right after.
    fn reset(&mut self) {
        let opts = vec![qbz_i18n::t("No folder")];
        *self = State {
            folder_options: opts,
            folder_ids: vec![String::new()],
            last_logged_percent: -1,
            source_options: source_labels(),
            station_options: station_labels(),
            ..State::default()
        };
    }

    /// The per-source reset a picker change performs: everything the OLD
    /// source put in the session goes, so a stale file or username can never
    /// be what a later Fetch reads.
    fn clear_source_inputs(&mut self) {
        self.picked_file_name.clear();
        self.file_bytes = None;
        self.file_name.clear();
        self.service_user.clear();
        self.lastfm_mode = 0;
        self.station_index = 0;
        self.lb_playlist_options.clear();
        self.lb_playlist_mbids.clear();
        self.lb_playlist_index = 0;
        self.lb_list_loading = false;
        self.url.clear();
        self.preview = None;
        self.preview_url.clear();
        self.locked_provider = None;
        self.active_provider.clear();
        self.show_preview = false;
        self.can_fetch = false;
    }

    fn clear_summary(&mut self) {
        self.summary_playlist.clear();
        self.summary_matched.clear();
        self.summary_skipped.clear();
        self.summary_duplicates.clear();
        self.summary_parts.clear();
    }

    fn clear_progress_lines(&mut self) {
        self.has_progress = false;
        self.status_line.clear();
        self.current_track.clear();
    }
}

/// The picker's five labels, in index order (`source_kind`).
fn source_labels() -> Vec<String> {
    vec![
        qbz_i18n::t("Streaming URL"),
        qbz_i18n::t("Playlist file"),
        qbz_i18n::t("JSON file"),
        qbz_i18n::t("ListenBrainz"),
        qbz_i18n::t("Last.fm"),
    ]
}

/// The three Last.fm station labels, in `LastFmStation::from_index` order.
fn station_labels() -> Vec<String> {
    vec![
        qbz_i18n::t("Play your library"),
        qbz_i18n::t("Play your mix"),
        qbz_i18n::t("Play your recommendations"),
    ]
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

/// Import generation (§1.8): bumped on every [`open`] and [`execute`]. Sink
/// events and task completions carry the generation they were spawned with; a
/// mismatch means the modal was reset for a fresh run, so the stale run may
/// only fire toast + folder assignment + sidebar refresh, never modal writes.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Bumped by every [`open`]; QML resets its local text mirrors on the change.
static RESET_SEQ: AtomicI64 = AtomicI64::new(0);

pub fn current_generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

fn bump_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// Process start, for the coalescing clock. `Instant` has no const ctor, so it
/// is latched on first use.
static CLOCK: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);

fn now_ms() -> u64 {
    CLOCK.elapsed().as_millis() as u64
}

static LAST_PUBLISH_MS: AtomicU64 = AtomicU64::new(0);
static FLUSH_PENDING: AtomicBool = AtomicBool::new(false);

/// Minimum gap between two PROGRESS-driven publishes (deviation 2 in the
/// module header). Log appends, phase changes and terminal arms bypass it.
const PROGRESS_PUBLISH_MS: u64 = 60;

fn build_doc(st: &State) -> ImportDoc {
    ImportDoc {
        open: st.open,
        reset_seq: RESET_SEQ.load(Ordering::SeqCst),
        url: st.url.clone(),
        custom_name: st.custom_name.clone(),
        loading: st.loading,
        error: st.error.clone(),
        active_provider: st.active_provider.clone(),
        can_fetch: st.can_fetch,
        show_preview: st.show_preview,
        import_completed: st.import_completed,
        progress_visible: st.progress_visible,
        has_progress: st.has_progress,
        progress: st.progress,
        status_line: st.status_line.clone(),
        current_track: st.current_track.clone(),
        log: st.log.clone(),
        source_index: st.source_index,
        source_options: st.source_options.clone(),
        picked_file_name: st.picked_file_name.clone(),
        service_user: st.service_user.clone(),
        lastfm_mode: st.lastfm_mode,
        station_options: st.station_options.clone(),
        station_index: st.station_index,
        lb_playlist_options: st.lb_playlist_options.clone(),
        lb_playlist_index: st.lb_playlist_index,
        lb_list_loading: st.lb_list_loading,
        folder_options: st.folder_options.clone(),
        folder_ids: st.folder_ids.clone(),
        folder_index: st.folder_index,
        summary_playlist: st.summary_playlist.clone(),
        summary_matched: st.summary_matched.clone(),
        summary_skipped: st.summary_skipped.clone(),
        summary_duplicates: st.summary_duplicates.clone(),
        summary_parts: st.summary_parts.clone(),
    }
}

/// Serialise the live state and post it to Qt. Never call this while holding
/// [`STATE`] — it takes the lock itself.
fn publish() {
    let doc = {
        let st = STATE.lock().unwrap();
        build_doc(&st)
    };
    LAST_PUBLISH_MS.store(now_ms(), Ordering::SeqCst);
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".into());
    crate::playlist_import_bridge::ui(move |mut b| {
        b.as_mut().set_import_json(QString::from(json.as_str()));
    });
}

/// Rate-limited publish for the high-frequency progress path, with a
/// GUARANTEED trailing flush so the last value of a burst is never the one
/// that got dropped.
fn publish_progress() {
    let now = now_ms();
    let last = LAST_PUBLISH_MS.load(Ordering::SeqCst);
    if now.saturating_sub(last) >= PROGRESS_PUBLISH_MS {
        publish();
        return;
    }
    // One flush in flight at a time; it publishes whatever the state holds
    // when it wakes, which is by definition the newest value.
    if FLUSH_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(PROGRESS_PUBLISH_MS)).await;
        FLUSH_PENDING.store(false, Ordering::SeqCst);
        publish();
    });
}

/// Append one pre-formatted line to the conversion log and publish
/// unconditionally (a log line is a discrete event, never coalesced).
fn push_log(message: String, status: &str) {
    {
        let mut st = STATE.lock().unwrap();
        st.log.push(LogEntry {
            message,
            status: status.to_string(),
        });
    }
    publish();
}

// ---------------------------------------------------------------------------
// Invokables
// ---------------------------------------------------------------------------

/// Open the modal fully reset (§5.8.3). Qt thread — nothing here touches the
/// DB; the folder list arrives from the spawned read below.
pub fn open() {
    // Invalidate any in-flight run's modal writes before resetting.
    bump_generation();
    RESET_SEQ.fetch_add(1, Ordering::SeqCst);
    {
        let mut st = STATE.lock().unwrap();
        st.reset();
        st.open = true;
        // The offline gate is recomputed here too, so a modal opened offline
        // starts with its confirm button already disabled rather than waiting
        // for the first keystroke.
        st.can_fetch = false;
    }
    publish();
    refresh_folders();
}

/// Read the folder list off library.db and republish. `!is_hidden` only
/// (§5.8.2 / D11) — a hidden folder is not offered as an import target, the
/// same rule the manager's move-to-folder menus follow.
fn refresh_folders() {
    crate::spawn(async move {
        let folders = tokio::task::spawn_blocking(|| {
            crate::folders_qt::load_folders_full()
                .into_iter()
                .filter(|f| !f.is_hidden)
                .map(|f| (f.id, f.name))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        {
            let mut st = STATE.lock().unwrap();
            // A modal closed while the read was in flight must not be
            // resurrected, and its reset already installed the "No folder"
            // pair.
            if !st.open {
                return;
            }
            st.folder_options = std::iter::once(qbz_i18n::t("No folder"))
                .chain(folders.iter().map(|(_, name)| name.clone()))
                .collect();
            st.folder_ids = std::iter::once(String::new())
                .chain(folders.iter().map(|(id, _)| id.clone()))
                .collect();
            st.folder_index = 0;
        }
        publish();
    });
}

/// Close. Deliberately does NOT cancel an in-flight import (§1.8) and does NOT
/// clear the state — a reopen is what resets, so a user who closes by accident
/// mid-import and reopens still sees... nothing of the old run, because
/// [`open`] resets. The state is kept only so the running task's completion
/// arms have somewhere to write.
pub fn close() {
    {
        let mut st = STATE.lock().unwrap();
        st.open = false;
    }
    publish();
}

/// Is the modal still on screen? Read by the completion arm before navigating.
fn is_open() -> bool {
    STATE.lock().map(|st| st.open).unwrap_or(false)
}

/// Recompute the URL-derived fields on every keystroke (the reference's
/// derived `detectedProvider` / `activeProvider` / `isValid` / `showPreview`),
/// plus the post-completion fresh-import rearm path.
/// "Does the ACTIVE source have enough input to Fetch?"
///
/// One rule per source, and the offline nuance is real rather than uniform:
/// FILE and JSON parse locally, so their preview works with no network at all
/// — which is the whole point for someone importing an M3U on a plane. Every
/// other source is a network read and is gated like the URL always was. The
/// IMPORT half needs a session regardless and is gated separately, so a file
/// can preview offline and still refuse to import.
fn compute_can_fetch(st: &State) -> bool {
    let offline = crate::offline_fwd::engine().is_offline();
    match st.source_index {
        source_kind::FILE | source_kind::JSON => st.file_bytes.is_some(),
        source_kind::LISTENBRAINZ => {
            !offline
                && match listenbrainz::detect(&st.service_user) {
                    // A username alone is not enough: one of its "created for
                    // you" playlists has to be chosen first.
                    Some(listenbrainz::LbTarget::User(_)) => !st.lb_playlist_mbids.is_empty(),
                    Some(listenbrainz::LbTarget::Mbid(_)) => true,
                    None => false,
                }
        }
        source_kind::LASTFM => !offline && lastfm::detect(&st.service_user).is_some(),
        // URL (and any unexpected index) keeps the original rule.
        _ => detect_provider_key(&st.url).is_some() && !offline,
    }
}

/// The source picker changed. Everything the previous source contributed is
/// dropped — see `clear_source_inputs`.
pub fn on_source_changed(index: i32) {
    // Read the stored handles BEFORE taking the lock — the store is a blocking
    // read and this runs on the Qt thread.
    let (lastfm_user, lb_user, _) = crate::integrations_qt::scrobbler_handles();
    let mut prefill_lb: Option<String> = None;
    {
        let mut st = STATE.lock().unwrap();
        if st.source_index == index {
            return;
        }
        st.source_index = index;
        st.clear_source_inputs();
        // PREFILL, never a requirement (integrations stay opt-in): a connected
        // account fills the field, an empty one leaves it blank and the source
        // still works with any public handle typed by hand.
        match index {
            source_kind::LASTFM => {
                st.service_user = lastfm_user;
                if lastfm::detect(&st.service_user).is_some() {
                    st.lastfm_mode = 0;
                }
            }
            source_kind::LISTENBRAINZ => {
                st.service_user = lb_user;
                if matches!(
                    listenbrainz::detect(&st.service_user),
                    Some(listenbrainz::LbTarget::User(_))
                ) {
                    prefill_lb = Some(st.service_user.clone());
                }
            }
            _ => {}
        }
        st.error.clear();
        st.import_completed = false;
        st.progress_visible = false;
        st.progress = 0.0;
        st.log.clear();
        st.clear_progress_lines();
        st.clear_summary();
        st.can_fetch = compute_can_fetch(&st);
    }
    publish();
    // A prefilled ListenBrainz username needs its "created for you" list, or
    // the picker would be empty until the user typed a character.
    if let Some(user) = prefill_lb {
        load_lb_created_for(user);
    }
}

/// The ListenBrainz / Last.fm handle field, on every keystroke.
///
/// ANY edit drops the preview: it is the service analogue of editing the URL
/// away from the one that was fetched, and without it the footer button would
/// still say "Import" for a playlist the field no longer names.
pub fn on_service_input_edited(text: &str) {
    let lb_user = {
        let mut st = STATE.lock().unwrap();
        st.service_user = text.to_string();
        st.preview = None;
        st.show_preview = false;
        st.error.clear();

        if st.source_index == source_kind::LASTFM {
            // Profile vs playlist drives the sub-UI: a playlist URL imports
            // directly, a profile offers the three stations.
            st.lastfm_mode = match lastfm::detect(text) {
                Some(lastfm::LastFmTarget::Playlist { .. }) => 1,
                _ => 0,
            };
        }

        // A ListenBrainz USERNAME needs its "created for you" list fetched
        // before anything can be picked. An MBID needs nothing.
        let want_list = st.source_index == source_kind::LISTENBRAINZ
            && matches!(
                listenbrainz::detect(text),
                Some(listenbrainz::LbTarget::User(_))
            );
        if !want_list {
            st.lb_playlist_options.clear();
            st.lb_playlist_mbids.clear();
            st.lb_playlist_index = 0;
            st.lb_list_loading = false;
        }
        st.can_fetch = compute_can_fetch(&st);
        if want_list {
            match listenbrainz::detect(text) {
                Some(listenbrainz::LbTarget::User(u)) => Some(u),
                _ => None,
            }
        } else {
            None
        }
    };
    publish();
    if let Some(user) = lb_user {
        load_lb_created_for(user);
    }
}

/// Fetch the "created for you" dropdown for a ListenBrainz username.
///
/// Fired per keystroke while the field holds a username, so it is guarded the
/// same way every other detached task here is: the generation it started under
/// must still be current, AND the field must still hold the username it was
/// started for. Without the second check a fast typist gets the list for a
/// prefix of what they typed.
fn load_lb_created_for(user: String) {
    {
        let mut st = STATE.lock().unwrap();
        st.lb_list_loading = true;
    }
    publish();
    let generation = current_generation();
    crate::spawn(async move {
        let token = crate::integrations_qt::scrobbler_handles().2;
        let res = listenbrainz::list_created_for(&user, token.as_deref()).await;
        if generation != current_generation() {
            return;
        }
        {
            let mut st = STATE.lock().unwrap();
            let still_ours = matches!(
                listenbrainz::detect(&st.service_user),
                Some(listenbrainz::LbTarget::User(ref u)) if *u == user
            );
            if !still_ours {
                return;
            }
            st.lb_list_loading = false;
            match res {
                Ok(list) => {
                    st.lb_playlist_options = list.iter().map(|p| p.title.clone()).collect();
                    st.lb_playlist_mbids = list.iter().map(|p| p.mbid.clone()).collect();
                    st.lb_playlist_index = 0;
                    if st.lb_playlist_options.is_empty() {
                        st.error = qbz_i18n::t_args("No playlists found for {}.", &[&user]);
                    }
                }
                Err(e) => {
                    log::warn!("[qbz-qt] listenbrainz createdfor {user} failed: {e}");
                    st.lb_playlist_options.clear();
                    st.lb_playlist_mbids.clear();
                }
            }
            st.can_fetch = compute_can_fetch(&st);
        }
        publish();
    });
}

/// Last.fm station picker.
pub fn set_station_index(index: i32) {
    {
        let mut st = STATE.lock().unwrap();
        st.station_index = index.clamp(0, 2);
        // A different station is a different playlist.
        st.preview = None;
        st.show_preview = false;
    }
    publish();
}

/// ListenBrainz "created for you" picker.
pub fn set_lb_playlist_index(index: i32) {
    {
        let mut st = STATE.lock().unwrap();
        if index < 0 || index as usize >= st.lb_playlist_mbids.len() {
            return;
        }
        st.lb_playlist_index = index;
        st.preview = None;
        st.show_preview = false;
    }
    publish();
}

/// "Choose file…" — the native picker for the File and JSON sources.
///
/// THE SIZE CHECK HAPPENS BEFORE THE READ. `metadata().len()` on the path,
/// then refuse; the crate re-checks the byte length as defense in depth, but by
/// then a 2 GB pick would already be in RAM. This is the one place the wall
/// actually protects the process.
pub fn pick_file() {
    let (json_mode, generation) = {
        let st = STATE.lock().unwrap();
        (st.source_index == source_kind::JSON, current_generation())
    };
    crate::spawn(async move {
        let dialog = rfd::AsyncFileDialog::new().set_title(&qbz_i18n::t("Choose file…"));
        let dialog = if json_mode {
            dialog.add_filter(&qbz_i18n::t("JSON file"), &["json"])
        } else {
            dialog.add_filter(
                &qbz_i18n::t("Playlist file"),
                &["m3u", "m3u8", "pls", "xspf"],
            )
        };
        let Some(handle) = dialog.pick_file().await else {
            return; // cancelled — no toast, no state change.
        };
        let path = handle.path().to_path_buf();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let read = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, PlaylistImportError> {
            let len = std::fs::metadata(&path)
                .map_err(|e| PlaylistImportError::Http(e.to_string()))?
                .len();
            if len as usize > MAX_IMPORT_BYTES {
                return Err(PlaylistImportError::FileTooLarge);
            }
            std::fs::read(&path).map_err(|e| PlaylistImportError::Http(e.to_string()))
        })
        .await;

        // A reopen while the dialog was up must not land a file on the fresh
        // modal (the same guard `fetch` carries).
        if generation != current_generation() {
            return;
        }
        {
            let mut st = STATE.lock().unwrap();
            // Any newly-picked file invalidates the previous preview.
            st.preview = None;
            st.show_preview = false;
            match read {
                Ok(Ok(bytes)) => {
                    st.error.clear();
                    st.file_bytes = Some(bytes);
                    st.file_name = name.clone();
                    st.picked_file_name = name;
                }
                Ok(Err(e)) => {
                    st.file_bytes = None;
                    st.file_name.clear();
                    st.picked_file_name.clear();
                    st.error = localize_import_error(&e);
                }
                Err(e) => {
                    log::warn!("[qbz-qt] file read task panicked: {e}");
                    st.file_bytes = None;
                    st.picked_file_name.clear();
                    st.error = qbz_i18n::t("Could not read the file.");
                }
            }
            st.can_fetch = compute_can_fetch(&st);
        }
        publish();
    });
}

/// A `PlaylistImportError` as something a user can read, in their language.
///
/// The semantic variants exist FOR this: matching a discriminant is what makes
/// a localized message possible at all, where the old `Parse(String)` would
/// have forced prefix-matching on an English string.
fn localize_import_error(e: &PlaylistImportError) -> String {
    match e {
        PlaylistImportError::UnrecognizedFormat => {
            qbz_i18n::t("Unrecognized file format — use XSPF, PLS, M3U/M3U8, or JSON.")
        }
        PlaylistImportError::HlsManifest => {
            qbz_i18n::t("This looks like a live-stream manifest, not a playlist.")
        }
        PlaylistImportError::EmptyPlaylist => qbz_i18n::t("No tracks found in the file."),
        PlaylistImportError::FileTooLarge => qbz_i18n::t("That file is too large to import."),
        PlaylistImportError::JsonShapeUnrecognized => {
            qbz_i18n::t("Could not recognize a track list in this JSON.")
        }
        other => other.to_string(),
    }
}

pub fn on_url_edited(text: &str) {
    let trimmed = text.trim().to_string();
    let detected = detect_provider_key(text);
    {
        let mut st = STATE.lock().unwrap();
        st.url = text.to_string();

        // After a completed import, editing the URL away from the imported one
        // rearms the modal for a fresh import without reopening.
        if st.import_completed && trimmed != st.last_imported_url {
            st.locked_provider = None;
            st.import_completed = false;
            st.error.clear();
            st.log.clear();
            st.progress_visible = false;
            st.progress = 0.0;
            st.clear_progress_lines();
            st.clear_summary();
        }

        let active = st.locked_provider.or(detected);
        st.active_provider = active.map(|p| p.as_str()).unwrap_or("").to_string();
        st.can_fetch = compute_can_fetch(&st);
        st.show_preview = st.preview.is_some() && trimmed == st.preview_url;
    }
    publish();
}

/// Keep the rename mirror fresh (read back by [`execute`]). Fired on every
/// keystroke of the name field.
pub fn on_name_edited(text: &str) {
    {
        let mut st = STATE.lock().unwrap();
        st.custom_name = text.to_string();
    }
    publish();
}

/// Folder dropdown selection. Out-of-range indices are ignored rather than
/// stored: the id lookup at execute time indexes this same vector.
pub fn set_folder_index(index: i32) {
    {
        let mut st = STATE.lock().unwrap();
        if index < 0 || index as usize >= st.folder_ids.len() {
            return;
        }
        st.folder_index = index;
    }
    publish();
}

// ---------------------------------------------------------------------------
// Step A — fetch the preview
// ---------------------------------------------------------------------------

/// Step A gate + reset. Returns the SOURCE to resolve, or `None` when the gate
/// fails.
///
/// It builds a whole [`PlaylistSource`] rather than a URL string, and that is
/// what makes the expansion additive: the snapshot is taken here, on the Qt
/// thread, under the lock — so the detached task never reads `STATE` again and
/// a reopen mid-fetch cannot change what it is fetching.
fn begin_fetch() -> Option<PlaylistSource> {
    let source = {
        let mut st = STATE.lock().unwrap();
        if st.loading || !st.can_fetch {
            return None;
        }
        let source = build_source(&st)?;
        st.preview = None;
        st.preview_url.clear();
        // The locked provider / logo row belongs to the URL source only; the
        // others have no provider logo to light up.
        if st.source_index == source_kind::URL {
            let detected = detect_provider_key(&st.url)?;
            st.locked_provider = Some(detected);
            st.active_provider = detected.as_str().to_string();
        } else {
            st.locked_provider = None;
            st.active_provider.clear();
        }
        st.loading = true;
        st.error.clear();
        st.show_preview = false;
        st.import_completed = false;
        st.progress = 0.0;
        st.clear_progress_lines();
        st.clear_summary();
        st.log.clear();
        st.progress_visible = true;
        source
    };
    push_log(qbz_i18n::t("Checking playlist link..."), "info");
    Some(source)
}

/// The active source as a resolvable value. Pure over the state — the caller
/// already holds the lock.
fn build_source(st: &State) -> Option<PlaylistSource> {
    match st.source_index {
        source_kind::FILE => {
            let bytes = st.file_bytes.clone()?;
            // Detection runs HERE rather than at parse time so a wrong pick
            // fails before the modal flips into step B.
            let format =
                qbz_playlist_import::sources::file::detect_format(&bytes, &st.file_name).ok()?;
            Some(PlaylistSource::File {
                format,
                bytes,
                filename: st.file_name.clone(),
            })
        }
        source_kind::JSON => Some(PlaylistSource::Json {
            bytes: st.file_bytes.clone()?,
            filename: st.file_name.clone(),
        }),
        source_kind::LISTENBRAINZ => {
            let token = crate::integrations_qt::scrobbler_handles().2;
            match listenbrainz::detect(&st.service_user)? {
                listenbrainz::LbTarget::Mbid(mbid) => {
                    Some(PlaylistSource::ListenBrainz { mbid, token })
                }
                listenbrainz::LbTarget::User(_) => {
                    let mbid = st
                        .lb_playlist_mbids
                        .get(st.lb_playlist_index.max(0) as usize)?
                        .clone();
                    Some(PlaylistSource::ListenBrainz { mbid, token })
                }
            }
        }
        source_kind::LASTFM => match lastfm::detect(&st.service_user)? {
            lastfm::LastFmTarget::Playlist { user, id } => Some(PlaylistSource::LastFmPlaylist {
                user,
                playlist_id: id,
            }),
            lastfm::LastFmTarget::Profile { user } => Some(PlaylistSource::LastFmStation {
                user,
                station: LastFmStation::from_index(st.station_index),
            }),
        },
        _ => Some(PlaylistSource::Url(st.url.clone())),
    }
}

/// Step A. The preview needs no session — only the execute does. And for FILE
/// and JSON it needs no network either, which is why `compute_can_fetch` lets
/// those two through offline.
pub fn fetch() {
    let Some(source) = begin_fetch() else {
        return;
    };
    // A reopen mid-fetch bumps the generation; the stale preview result must
    // not land on the fresh modal (§1.8).
    let generation = current_generation();
    crate::spawn(async move {
        let label = source.label();
        let res = source.resolve().await;
        if generation != current_generation() {
            return;
        }
        match res {
            Ok(p) => apply_preview_ok(&label, p),
            Err(e) => apply_preview_err(&localize_import_error(&e)),
        }
    });
}

/// Preview fetch succeeded.
fn apply_preview_ok(label: &str, preview: ImportPlaylist) {
    let count = preview.tracks.len();
    let provider = provider_display_name(&preview.provider);
    {
        let mut st = STATE.lock().unwrap();
        st.custom_name = preview.name.clone();
        st.preview_url = label.trim().to_string();
        st.preview = Some(preview);
    }
    push_log(
        qbz_i18n::t_args("Found {} tracks from {}.", &[&count.to_string(), provider]),
        "success",
    );
    {
        let mut st = STATE.lock().unwrap();
        st.loading = false;
        // SOURCE-AWARE, and this is the line that makes the whole expansion
        // reachable. The URL rule is "the field still holds what was fetched",
        // which is right for a URL and impossible for the others: their URL
        // field is hidden and permanently `""`, so `"" == label` would be false
        // forever and the footer button would say "Fetch" for the rest of time
        // — Import unreachable for every new source.
        //
        // For a non-URL source the preview that just landed IS the current one:
        // any edit to the file or the handle clears it (`pick_file`,
        // `on_service_input_edited`), so there is nothing for an equality check
        // to protect against.
        st.show_preview = if st.source_index == source_kind::URL {
            st.url.trim() == st.preview_url
        } else {
            true
        };
    }
    publish();
}

/// Preview fetch failed.
fn apply_preview_err(err: &str) {
    {
        let mut st = STATE.lock().unwrap();
        st.error = err.to_string();
        st.loading = false;
    }
    push_log(qbz_i18n::t_args("Import failed: {}", &[err]), "error");
}

// ---------------------------------------------------------------------------
// Step B — execute the import
// ---------------------------------------------------------------------------

/// Everything the execute task needs, snapshotted before it spawns.
struct ExecuteArgs {
    /// THE RESOLVED PLAYLIST, cloned out of the session under the lock.
    ///
    /// It used to be the URL, and the task re-fetched from it. That was a
    /// double scrape on every URL import, and for the new sources it is not
    /// merely wasteful but impossible: the `rfd` bytes of a picked file are
    /// gone, and the dialog cannot silently reopen. Snapshotting also closes a
    /// real race — `open()` resets the session, so a modal reopened between the
    /// Import click and the task's read would have handed a detached task
    /// `None` and imported nothing.
    playlist: ImportPlaylist,
    name_override: Option<String>,
    /// Local folder id chosen in the dropdown (`""` = no folder).
    folder_id: String,
    /// The run's generation (§1.8), carried by the sink and the completion
    /// arms.
    generation: u64,
}

/// Step B gate + reset (the reference's `begin_execute`).
fn begin_execute() -> Option<ExecuteArgs> {
    let mut st = STATE.lock().unwrap();
    if st.loading || st.import_completed {
        return None;
    }
    let playlist = st.preview.as_ref()?.clone();
    let source_name = playlist.name.clone();
    // The rename goes out only when it differs from the source name; an empty
    // rename falls back to the source name (reference Appendix A).
    let custom = st.custom_name.trim().to_string();
    let name_override = if custom != source_name && !custom.is_empty() {
        Some(custom)
    } else {
        None
    };
    st.last_logged_percent = -1;
    let folder_id = st
        .folder_ids
        .get(st.folder_index.max(0) as usize)
        .cloned()
        .unwrap_or_default();
    st.loading = true;
    st.error.clear();
    st.progress_visible = true;
    Some(ExecuteArgs {
        playlist,
        name_override,
        folder_id,
        generation: bump_generation(),
    })
}

/// Step B — `import_public_playlist(...)` with live sink progress, then the
/// folder assignment, the toast, the sidebar reconcile and the navigation.
pub fn execute() {
    let Some(args) = begin_execute() else {
        return;
    };
    publish();
    let runtime = crate::app();
    crate::spawn(async move {
        // The reference's `RequiresUserSession` gate: execute needs a
        // logged-in client (the preview does not).
        let client = {
            let lock = runtime.core().client();
            let guard = lock.read().await;
            guard.as_ref().cloned()
        };
        let Some(client) = client else {
            if args.generation == current_generation() {
                apply_execute_err(&qbz_i18n::t("Not logged in to Qobuz"));
            }
            crate::toast_qt::error(qbz_i18n::t("Playlist import failed"));
            return;
        };

        let sink: Arc<dyn ImportProgressSink> = Arc::new(QtSink {
            generation: args.generation,
        });
        let res = qbz_playlist_import::import_prepared_playlist(
            args.playlist,
            &client,
            args.name_override.as_deref(),
            false, // is_public — the reference hardcodes false, no toggle
            sink,
        )
        .await;

        match res {
            Ok(summary) => {
                // reco: NOT logged, 1:1 with the reference — the importer is a
                // bulk external import, not a per-track taste action. (This
                // port has no reco store at all; see the picker's OWED PORT.)
                //
                // Assign every created part to the chosen folder (library.db)
                // BEFORE the sidebar reload, so the reload already sees the
                // membership.
                if !args.folder_id.is_empty() {
                    for pid in &summary.qobuz_playlist_ids {
                        let (pid, fid) = (*pid, args.folder_id.clone());
                        let _ = tokio::task::spawn_blocking(move || {
                            crate::folders_qt::move_playlist(pid, Some(fid.as_str()));
                        })
                        .await;
                    }
                }
                // Toast + sidebar refresh fire even after a close mid-import
                // (§1.8); the generation guard keeps a stale run's writes off a
                // reopened modal's fresh state.
                if args.generation == current_generation() {
                    apply_execute_ok(&summary);
                }
                if summary.matched_tracks > 0 {
                    crate::toast_qt::success(qbz_i18n::t("Playlist imported"));
                }
                reconcile_sidebar_after_import(
                    &runtime,
                    summary.qobuz_playlist_ids.clone(),
                    summary.playlist_name.clone(),
                    summary.matched_tracks,
                );
                if let Some(first) = summary.qobuz_playlist_ids.first() {
                    // Navigate only while the modal is still open AND this run
                    // is current (§1.8).
                    if args.generation == current_generation() && is_open() {
                        crate::open_playlist(first.to_string());
                    }
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if args.generation == current_generation() {
                    apply_execute_err(&msg);
                }
                crate::toast_qt::error(qbz_i18n::t("Playlist import failed"));
            }
        }
    });
}

/// Import finished: completion logs + the summary block + `importCompleted`.
fn apply_execute_ok(summary: &ImportSummary) {
    {
        let mut st = STATE.lock().unwrap();
        st.last_imported_url = st.preview_url.clone();
        st.import_completed = true;
    }
    push_log(
        qbz_i18n::t_args(
            "Imported {} of {} tracks into QBZ.",
            &[
                &summary.matched_tracks.to_string(),
                &summary.total_tracks.to_string(),
            ],
        ),
        "success",
    );
    if !summary.qobuz_playlist_ids.is_empty() {
        if summary.parts_created > 1 {
            push_log(parts_line(summary.parts_created), "success");
        } else {
            push_log(qbz_i18n::t("Playlist created in Qobuz™."), "success");
        }
    } else {
        push_log(qbz_i18n::t("No matching tracks found."), "error");
    }
    {
        let mut st = STATE.lock().unwrap();
        // Summary block (pre-formatted; "" = hidden). `playlist_name` is the
        // name the playlist was created under — rename included.
        st.summary_playlist = qbz_i18n::t_args("Playlist: {}", &[&summary.playlist_name]);
        st.summary_matched = qbz_i18n::t_args(
            "Tracks matched: {} / {}",
            &[
                &summary.matched_tracks.to_string(),
                &summary.total_tracks.to_string(),
            ],
        );
        st.summary_skipped =
            qbz_i18n::t_args("Skipped: {}", &[&summary.skipped_tracks.to_string()]);
        // Hidden when there were none, which is every well-formed source.
        st.summary_duplicates = if summary.duplicate_tracks > 0 {
            qbz_i18n::t_args("Duplicates: {}", &[&summary.duplicate_tracks.to_string()])
        } else {
            String::new()
        };
        st.summary_parts = if summary.parts_created > 1 {
            parts_line(summary.parts_created)
        } else {
            String::new()
        };
        // The bar/status hide with `loading`, as in the reference.
        st.clear_progress_lines();
        st.loading = false;
    }
    publish();
}

/// Import failed. The error toast lives in the caller.
fn apply_execute_err(err: &str) {
    {
        let mut st = STATE.lock().unwrap();
        st.error = err.to_string();
        st.clear_progress_lines();
        st.loading = false;
    }
    push_log(qbz_i18n::t_args("Import failed: {}", &[err]), "error");
}

// ---------------------------------------------------------------------------
// The progress sink
// ---------------------------------------------------------------------------

/// Streams crate events onto the document. Unlike the reference's `SlintSink`
/// there is no window handle to upgrade: `publish` hops through the bridge's
/// `ui()` on its own, so `emit` is callable from the import task directly.
struct QtSink {
    generation: u64,
}

impl ImportProgressSink for QtSink {
    fn emit(&self, event: ImportEvent) {
        // Stale generation = the modal was reset (reopened) while this run was
        // in flight — its events must never touch the fresh state (§1.8).
        if self.generation != current_generation() {
            return;
        }
        apply_event(event);
    }
}

/// One sink event onto the document — the two Svelte event listeners.
fn apply_event(event: ImportEvent) {
    match event {
        ImportEvent::Phase(phase) => match phase {
            ImportPhase::Matching => {
                push_log(qbz_i18n::t("Searching Qobuz catalog..."), "info");
            }
            // Creating / Adding re-fire once per created part — log each, as
            // the reference does.
            ImportPhase::Creating => {
                push_log(qbz_i18n::t("Creating playlist..."), "success");
            }
            ImportPhase::Adding => {
                push_log(qbz_i18n::t("Adding tracks to playlist..."), "info");
            }
        },
        ImportEvent::Progress(p) => {
            // The bar + the status line update on EVERY event; only the
            // PUBLISH is coalesced (module header, deviation 2).
            let mut log_line: Option<String> = None;
            {
                let mut st = STATE.lock().unwrap();
                st.has_progress = p.total > 0;
                if p.total > 0 {
                    st.progress = p.current as f32 / p.total as f32;
                }
                if p.phase == "adding" {
                    // Status line per phase — the owner's deliberate deviation
                    // from the Tauri modal, which reused "Matching tracks…"
                    // here (see qbz_playlist_import::sink::ImportPhase).
                    let line = qbz_i18n::t_args(
                        "Adding tracks: {} / {}",
                        &[&p.current.to_string(), &p.total.to_string()],
                    );
                    st.status_line = line.clone();
                    // One log line per 50-track chunk event (chunk counts, not
                    // tracks) — the reference logs every adding event.
                    log_line = Some(line);
                } else if p.total > 0 {
                    let line = qbz_i18n::t_args(
                        "Matching tracks: {} / {} ({} found)",
                        &[
                            &group_thousands(p.current),
                            &group_thousands(p.total),
                            &group_thousands(p.matched_so_far),
                        ],
                    );
                    st.status_line = line.clone();
                    // Matching is high-frequency (one event per track): log
                    // only at 5 % milestones, exactly like the Svelte listener.
                    let pct = (p.current as u64 * 100 / p.total as u64) as i32;
                    if pct >= st.last_logged_percent + 5 {
                        st.last_logged_percent = pct;
                        log_line = Some(line);
                    }
                }
                st.current_track = p.current_track.unwrap_or_default();
            }
            match log_line {
                // `push_log` publishes unconditionally, which is what keeps a
                // log append from being swallowed by the coalescing window.
                Some(line) => push_log(line, "info"),
                None => publish_progress(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sidebar reconcile (the reference's `main.rs::reconcile_sidebar_after_import`)
// ---------------------------------------------------------------------------

/// Make the imported playlist appear in the sidebar despite the user-playlists
/// endpoint lagging the write: an OPTIMISTIC row now, then a bounded-retry
/// reload until the API lists it.
///
/// `sidebar_qt::load` + `crate::publish_sidebar()` are called directly rather
/// than `crate::reload_sidebar()`: the latter early-returns while offline, and
/// the pair below is the refresh that is correct in every connectivity state
/// (the same reasoning D10 records for the manager's optimistic move).
fn reconcile_sidebar_after_import(
    runtime: &Runtime,
    ids: Vec<u64>,
    name: String,
    tracks_count: u32,
) {
    const MAX_ATTEMPTS: u32 = 6;
    if ids.is_empty() {
        return;
    }
    let first = ids[0];
    let single = ids.len() == 1;
    // Optimistic insert NOW (single-playlist imports only — multi-part imports
    // get their per-part names from the API once it catches up).
    if single {
        // No covers to give: the import created the playlist seconds ago and
        // nothing has resolved its collage yet (the next real load fills it).
        crate::sidebar_qt::insert_qobuz_entry(first, &name, tracks_count, &[]);
        crate::publish_sidebar();
    }
    let runtime = runtime.clone();
    crate::spawn(async move {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            crate::sidebar_qt::load(&runtime).await;
            let present = ids
                .iter()
                .all(|id| crate::sidebar_qt::playlist_name(*id).is_some());
            // Put the optimistic row BACK after every load, not only at the
            // terminal attempt. This port's `sidebar_qt::load` REPLACES the
            // cache in place, where the reference's `sidebar::load` returns its
            // data and only `sidebar::apply` writes — so there the optimistic
            // row survives the whole retry window and here it would not. Any
            // unrelated `publish_sidebar()` inside that window (a folder
            // toggle, a sort or search change, a local-playlist mutation) would
            // otherwise republish a cache the imported playlist had just been
            // wiped out of, and the row would vanish for up to 15 s. Idempotent
            // by id, and evaluated AFTER `present` so it cannot fake it.
            if !present && single {
                crate::sidebar_qt::insert_qobuz_entry(first, &name, tracks_count, &[]);
            }
            if present || attempt >= MAX_ATTEMPTS {
                if !present {
                    log::warn!(
                        "[qbz-qt] playlist-import: sidebar list still missing the imported \
                         playlist after {attempt} attempts; the optimistic row stays until \
                         the next load"
                    );
                }
                crate::publish_sidebar();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(attempt as u64)).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Formatting helpers (ported verbatim)
// ---------------------------------------------------------------------------

/// "Split into {count} playlists (Qobuz 2000-track limit)" — used as both a log
/// line and the summary parts line, as in the reference.
fn parts_line(count: u32) -> String {
    qbz_i18n::t_args(
        "Split into {} playlists (Qobuz 2000-track limit)",
        &[&count.to_string()],
    )
}

/// Display names for the "Found N tracks from {provider}." log (Svelte
/// `formatProvider`). The enum is exhaustive, so Svelte's "Unknown" arm is
/// unreachable here.
fn provider_display_name(provider: &ImportProvider) -> &'static str {
    match provider {
        ImportProvider::Spotify => "Spotify",
        ImportProvider::AppleMusic => "Apple Music",
        ImportProvider::Tidal => "Tidal",
        ImportProvider::Deezer => "Deezer",
        // The four added by the 2.0.3 expansion. "Playlist file" and
        // "JSON file" are deliberately the SAME strings as the source picker's
        // labels, so "Found 42 tracks from JSON file." and the picker read as
        // one vocabulary.
        ImportProvider::File => "Playlist file",
        ImportProvider::Json => "JSON file",
        ImportProvider::ListenBrainz => "ListenBrainz",
        ImportProvider::LastFm => "Last.fm",
    }
}

/// `toLocaleString()` twin for the matching log/status numbers ("12,345").
/// Fixed en-US grouping is the deliberate choice, 1:1 with the reference.
fn group_thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::group_thousands;

    #[test]
    fn group_thousands_matches_to_locale_string() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(12345), "12,345");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }
}
