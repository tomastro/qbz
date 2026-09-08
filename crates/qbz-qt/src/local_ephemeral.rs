//! Ephemeral folders and live disc sessions.
//!
//! A folder opened outside the indexed library, a physical CD and a manually
//! opened SACD image never land a row in `library.db`. SACD images become
//! persistent only when the regular folder scanner finds them below a
//! registered Local Library root.
//!
//! Qt/QML port of the shipping Slint pair (`crates/qbz/src/ephemeral.rs` +
//! the `EphemeralPane` arm of `crates/qbz/src/local_library.rs`). ADR-006:
//! NOTHING here re-implements scanning, CUE explosion, artwork extraction or
//! album grouping — all of that is `qbz_library::ephemeral`, the same
//! frontend-agnostic module the Slint binary drives. This file owns three
//! things only:
//!
//!  1. the process-global session handle (in-memory, dies with the process);
//!  2. the ONE JSON document the QML pane parses
//!     (`{name, path, trackCount, multiAlbum, albums:[…]}`), built out of the
//!     SAME `local_rows::TrackRow` every other Local Library surface uses;
//!  3. the play seam — the queue is mapped by `local_playback`'s
//!     `local_queue_track` and handed to the SHARED audible step
//!     (`audible_qt`, design 02 §9 stage 3). This file used to carry its own
//!     `play_file`, described in its own doc as "`local_playback::
//!     play_local_file` 1:1 apart from that lookup"; the lookup is now
//!     `LocalSource::track_row`, which reads this session store for any
//!     ephemeral id, so there is no ephemeral arm anywhere in playback. The
//!     PROTECTED audio path is entered there, never modified.
//!
//! Ephemeral track ids are SYNTHETIC and high (`>= 2^48`, the shared
//! `EPHEMERAL_ID_FLOOR`), so they can never collide with a `local_tracks` row
//! id: `is_ephemeral_id` is what lets any playback caller route an id to the
//! in-memory store instead of the DB.
//!
//! What SURVIVES a restart is the folder PATH only (`locallibrary_ui.json`'s
//! `ephemeral_folder`, shared with the Slint frontend); `rehydrate()` re-scans
//! it at boot.
//!
//! FILE PICKERS use `rfd::AsyncFileDialog` — the platform's own dialog. They
//! used to shell out to zenity / qarma / kdialog / yad under a POC-NOTE that
//! said swapping in `rfd` was "a five-line change once the dependency exists".
//! The dependency arrived (Settings > Local Library has called it from this
//! same crate for weeks) and nobody came back for the note, so on macOS —
//! where none of those four binaries exist — `Open folder…` and `Open SACD
//! image…` opened nothing at all, in silence, while the Settings picker two
//! menus away worked fine. Fixed 2026-08-21, on the owner's Mac mini. A stale
//! note is not a comment; it is a downgrade with an excuse attached.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use cxx_qt_lib::QString;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
use qbz_library::ephemeral::{EphemeralLibraryState, EPHEMERAL_ID_FLOOR};
use qbz_library::LocalTrack;
use qbz_models::QueueTrack;
use serde::Serialize;

use crate::local_bridge::ui;
use crate::local_rows::{album_key, map_track, to_json, TrackRow};
use crate::local_state::{read_prefs, with_art, write_prefs};

type Runtime = Arc<AppRuntime<LoggingAdapter>>;

// ---------------------------------------------------------------------------
// Session store (the Slint `crates/qbz/src/ephemeral.rs` 1:1)
// ---------------------------------------------------------------------------

// Every method on `EphemeralLibraryState` takes `&self` and guards its own
// inner Mutex, so a bare LazyLock (no outer lock) is enough.
static STATE: LazyLock<EphemeralLibraryState> = LazyLock::new(EphemeralLibraryState::new);

/// True when `id` is a synthetic ephemeral track id. DB row ids never reach
/// the floor, so this check unambiguously routes a playback request to the
/// in-memory store instead of `library.db`.
pub fn is_ephemeral_id(id: i64) -> bool {
    id >= EPHEMERAL_ID_FLOOR
}

/// Resolve a synthetic id to its cached row (None once the session is gone).
pub fn get_track(id: i64) -> Option<LocalTrack> {
    STATE.get_track(id)
}

/// True when the open session is a physical CD — the only medium this app
/// can rip, since an image is already a file.
pub fn session_is_cd() -> bool {
    STATE
        .tracks_snapshot()
        .first()
        .map(|t| qbz_disc::CdRef::is_cd_path(&t.file_path))
        .unwrap_or(false)
}

/// Where to put a rip. A folder chooser, and the answer is never guessed.
pub(crate) async fn pick_folder_for_rip() -> Option<String> {
    pick_dir(
        &qbz_i18n::t("Where should the ripped album go?"),
        dirs::audio_dir(),
    )
    .await
}

/// The one folder chooser, for every caller that needs one.
///
/// `rfd` gives the platform's OWN dialog — the Portal on Wayland, NSOpenPanel
/// on macOS — instead of shelling out to whichever of zenity / qarma /
/// kdialog / yad happens to be installed.
///
/// THAT SHELL-OUT IS WHY THIS EXISTS. It was written when this crate had no
/// `rfd` dependency, under a POC-NOTE that said swapping it in "is a five-line
/// change once the dependency exists". The dependency arrived — Settings >
/// Local Library has been calling `rfd::AsyncFileDialog` from this same crate
/// for weeks — and nobody came back for the note. On macOS none of those four
/// binaries exist, so `Open folder…` and `Open SACD image…` opened NOTHING,
/// silently, while the Settings picker two menus away worked fine. A stale
/// note is not a comment; it is a downgrade with an excuse attached.
///
/// ASYNC, not blocking: `rfd::FileDialog`'s blocking API must run on the main
/// thread on macOS, and every caller here is on a worker. The async one posts
/// itself to the right thread, which is also what the Settings path does.
async fn pick_dir(title: &str, start: Option<PathBuf>) -> Option<String> {
    let start = start
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));
    let handle = rfd::AsyncFileDialog::new()
        .set_title(title)
        .set_directory(&start)
        .pick_folder()
        .await?;
    Some(handle.path().to_string_lossy().into_owned())
}

/// Every track of the current session, in scan (= display) order.
pub fn tracks_snapshot() -> Vec<LocalTrack> {
    STATE.tracks_snapshot()
}

/// The album grouping key for one ephemeral row — `album_group_key` when set,
/// else `album|||album_artist`. Mirrors the scanner's own fallback so the
/// pane's grouping and the play-album lookup can never disagree.
/// "Disc 2", translated. Its own function so the eight catalogues carry ONE
/// msgid rather than a formatted string built two ways.
fn disc_label(n: u32) -> String {
    qbz_i18n::t_args("Disc {}", &[&n.to_string()])
}

fn album_key_of(t: &LocalTrack) -> String {
    let album = if !t.album_group_key.is_empty() {
        t.album_group_key.clone()
    } else {
        format!(
            "{}|||{}",
            t.album,
            t.album_artist.as_deref().unwrap_or(&t.artist)
        )
    };
    // AND THE DISC. A box set scans as ONE album — measured on the owner's
    // Saint Seiya Eternal CD-Box, where `album_group_key` is the box's root
    // folder for all 34 tracks and the disc number sits right there in the
    // data, unused. Without this the pane is a single flat list 247 rows long,
    // which is not a browsable thing; with it each disc is its own block with
    // its own header and play button.
    //
    // Unconditional, including for a single-disc album (which keys as
    // `…#disc1`), because the key ROUND-TRIPS: `album_tracks` looks a block up
    // by re-deriving it per track, so a key built one way here and another way
    // there is a play button that plays nothing.
    format!("{album}#disc{}", t.disc_number.unwrap_or(1))
}

/// The tracks of one album block, in scan order.
///
/// `pub(crate)` under a second name for `source_wiring::EphemeralGlue`: the
/// seam's `LocalSource` resolves an ephemeral album through this store rather
/// than through `library.db`, and it needs the same scan order the pane shows.
pub(crate) fn album_tracks_for(group_key: &str) -> Vec<LocalTrack> {
    album_tracks(group_key)
}

/// Snapshot one physical directory for the app-wide metadata editor.
///
/// The pane splits a multidisc directory into visual blocks, but metadata and
/// its `.qbz.json` sidecar are directory-scoped. Expanding the clicked block
/// back to every row in that directory avoids overwriting a sibling disc's
/// sidecar entries or deleting them after a verified direct write.
pub(crate) fn editor_snapshot(group_key: &str) -> Option<(String, String, Vec<LocalTrack>)> {
    let selected = album_tracks(group_key);
    let first = selected.first()?;
    let directory = Path::new(&first.file_path).parent()?.to_path_buf();
    if !directory.is_dir() {
        return None;
    }
    let tracks = STATE
        .tracks_snapshot()
        .into_iter()
        .filter(|track| Path::new(&track.file_path).parent() == Some(directory.as_path()))
        .collect::<Vec<_>>();
    if tracks.is_empty() {
        return None;
    }
    Some((
        STATE.current_folder_path()?,
        directory.to_string_lossy().into_owned(),
        tracks,
    ))
}

/// Publish metadata edits into the live ephemeral session without renumbering
/// rows already referenced by the queue. The library state performs the
/// session and exact id/path checks atomically before replacing anything.
pub(crate) fn apply_editor_update(
    expected_session: &str,
    edited: &[LocalTrack],
) -> Result<(), qbz_library::LibraryError> {
    let tracks = STATE
        .replace_tracks_for_session(expected_session, edited)
        .map_err(|error| qbz_library::LibraryError::Metadata(error.to_string()))?
        .ok_or_else(|| {
            qbz_library::LibraryError::Metadata(qbz_i18n::t(
                "The ephemeral session changed; reopen the metadata editor.",
            ))
        })?;
    let name = if Path::new(expected_session).is_dir() {
        folder_display_name(expected_session)
    } else {
        expected_session.to_string()
    };
    publish_doc(&build_doc(&name, expected_session, &tracks), false);
    Ok(())
}

fn album_tracks(group_key: &str) -> Vec<LocalTrack> {
    STATE
        .tracks_snapshot()
        .into_iter()
        .filter(|t| album_key_of(t) == group_key)
        .collect()
}

// ---------------------------------------------------------------------------
// The document (the QML contract)
// ---------------------------------------------------------------------------

/// `{name, path, trackCount, multiAlbum, albums:[…]}` — the whole pane in one
/// publish, exactly like every other Local Library surface.
#[derive(Serialize)]
struct EphemeralDoc {
    /// Folder/device name retained as the medium identity.
    name: String,
    /// Consistent tagged album/collection title, else `name`.
    title: String,
    /// `name` when `title` came from track metadata and adds useful context;
    /// empty for a mixed folder or when both strings are effectively equal.
    #[serde(rename = "folderContext")]
    folder_context: String,
    path: String,
    #[serde(rename = "trackCount")]
    track_count: usize,
    /// Total playing time, already formatted ("1 h 12 min" / "42 min") by the
    /// SAME helper every other Local Library surface uses, so the pane header
    /// and an album card can never render the same duration two ways.
    #[serde(rename = "totalDuration")]
    total_duration: String,
    /// > 1 album in the folder: the per-album play button appears only then.
    #[serde(rename = "multiAlbum")]
    multi_album: bool,
    albums: Vec<EphemeralAlbumBlock>,
}

/// One album block: header (cover + title/artist/meta + CUE badge) + tracks.
#[derive(Serialize)]
struct EphemeralAlbumBlock {
    #[serde(rename = "groupKey")]
    group_key: String,
    /// Which disc this block is, for ordering. A box's blocks otherwise tie on
    /// title and fall back to the lexicographic order of their string keys.
    #[serde(skip)]
    disc: u32,
    /// The ALBUM's title, without the per-disc name. The sort key, so the
    /// blocks of one box stay together and in disc order instead of being
    /// scattered by what each disc happens to be called.
    #[serde(skip)]
    sort_title: String,
    title: String,
    artist: String,
    /// "1998 · 12 tracks" (already formatted + translated).
    meta: String,
    /// Single-file CUE rip (one audio file, virtual tracks).
    #[serde(rename = "isCue")]
    is_cue: bool,
    #[serde(rename = "artKey")]
    art_key: String,
    tracks: Vec<TrackRow>,
}

/// Last path segment (the header's folder name).
fn folder_display_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn normalized_title(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn consistent_title(values: impl Iterator<Item = String>) -> Option<String> {
    let mut first: Option<(String, String)> = None;
    for value in values {
        let display = value.trim();
        if display.is_empty() || display.eq_ignore_ascii_case("Unknown Album") {
            return None;
        }
        let normalized = normalized_title(display);
        match first.as_ref() {
            Some((_, expected)) if expected != &normalized => return None,
            Some(_) => {}
            None => first = Some((display.to_string(), normalized)),
        }
    }
    first.map(|(display, _)| display)
}

/// Infer a real collection title only when every playable row agrees. The
/// group title is preferred because it intentionally removes bare "Disc N"
/// suffixes; the raw album tag is a conservative fallback. A user's folder of
/// unrelated tracks therefore keeps the folder name instead of inventing an
/// album from the first row.
fn consistent_collection_title(tracks: &[LocalTrack]) -> Option<String> {
    if tracks.is_empty() {
        return None;
    }
    consistent_title(tracks.iter().map(|track| track.album_group_title.clone())).or_else(|| {
        consistent_title(
            tracks.iter().map(|track| {
                qbz_library::MetadataExtractor::strip_disc_suffix_public(&track.album)
            }),
        )
    })
}

/// Group the scanned rows into album blocks (sorted by title) and register
/// every cover in the shared art index, so the pane's covers ride the SAME
/// windowed `artworkWindow` -> `localArtworkReady` path as the grids.
/// Cheap (no I/O) but it takes the local-state lock — never call it on the Qt
/// thread.
fn build_doc(name: &str, path: &str, tracks: &[LocalTrack]) -> EphemeralDoc {
    let collection_title = consistent_collection_title(tracks);
    let title = collection_title.clone().unwrap_or_else(|| name.to_string());
    let folder_context = collection_title
        .filter(|album| normalized_title(album) != normalized_title(name))
        .map(|_| name.to_string())
        .unwrap_or_default();
    // BTreeMap keeps a deterministic key order; scan order is preserved
    // inside each group (the scanner already sorted album/disc/track/title).
    let mut groups: BTreeMap<String, Vec<&LocalTrack>> = BTreeMap::new();
    for t in tracks {
        groups.entry(album_key_of(t)).or_default().push(t);
    }
    let multi_album = groups.len() > 1;
    // More than one DISC anywhere in the session — what decides whether a
    // block's meta line names its disc.
    let multi_disc = tracks
        .iter()
        .map(|t| t.disc_number.unwrap_or(1))
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        > 1;
    let mut albums: Vec<EphemeralAlbumBlock> = with_art(|art| {
        groups
            .into_iter()
            .map(|(key, group)| {
                let first = group[0];
                let group_title = if first.album_group_title.is_empty() {
                    first.album.clone()
                } else {
                    first.album_group_title.clone()
                };
                // A BOX NAMES ITS DISCS, so a block that IS a disc says which.
                // `album_group_title` is the group name with the disc suffix
                // stripped, identical for all 13 blocks of a box — which is
                // why every heading used to read "Saint Seiya Eternal CD-Box"
                // (owner report 2026-08-22, with a screenshot of the Folders
                // grid getting it right from the same files).
                let disc_name = crate::local_rows::disc_display_title(first, &group_title);
                let title = if disc_name.is_empty() {
                    group_title.clone()
                } else {
                    format!("{group_title} — {disc_name}")
                };
                let artist = first
                    .album_artist
                    .clone()
                    .unwrap_or_else(|| first.artist.clone());
                let count = group.len();
                let tracks_label =
                    qbz_i18n::tf("{} track", "{} tracks", count as i64, &[&count.to_string()]);
                // "2008 · Disc 2 · 17 tracks". The disc is named ONLY when the
                // session actually holds more than one — on an ordinary album
                // "Disc 1" is noise that distinguishes nothing.
                let disc = first.disc_number.unwrap_or(1);
                let meta = match (first.year, multi_disc) {
                    (Some(y), true) if y > 0 => {
                        format!("{y} · {} · {tracks_label}", disc_label(disc))
                    }
                    (Some(y), false) if y > 0 => format!("{y} · {tracks_label}"),
                    (_, true) => format!("{} · {tracks_label}", disc_label(disc)),
                    (_, false) => tracks_label,
                };
                // Namespaced so an ephemeral block can never claim the art key
                // of an indexed album that happens to group the same — and it
                // names the COVER, not just the album.
                //
                // Naming the cover is load-bearing, not tidiness. A disc's art
                // arrives LATE (the Cover Art Archive took 9.4 s), and the only
                // thing `set_session_artwork` can change is this document: the
                // path itself rides the side channel, never the JSON. With the
                // key fixed, the republished document was BYTE-IDENTICAL, and
                // cxx-qt 0.7's generated setter drops an equal value without
                // emitting `Changed` ("don't want to set the value again and
                // reemit the signal, as this can cause binding loops",
                // cxx-qt-gen/src/generator/rust/property/setter.rs:74). So the
                // view never re-reported its window, never asked for the key a
                // second time, and the cover sat in the index unrequested —
                // missing on every surface at once. A key that does not change
                // when the thing it names changes is a key that lies.
                // THIS disc's own cover. `artwork_path` is biased to the album
                // ROOT by the scanner, so all 13 blocks resolved to one image.
                // `disc_cover_url` reads the disc's own folder; it falls back
                // to the scanned value for anything that is not a titled disc
                // folder.
                let own = crate::local_rows::disc_cover_url(first);
                let own_path = crate::artwork_qt::local_path(&own);
                let cover = if own.is_empty() {
                    first.artwork_path.as_deref().filter(|p| !p.is_empty())
                } else {
                    Some(own_path.as_str())
                };
                let art_key = match cover {
                    Some(p) => album_key(&format!("eph:{key}:{p}")),
                    None => album_key(&format!("eph:{key}")),
                };
                if let Some(p) = cover {
                    if let Some(t) = crate::local_rows::art_token(first.source.as_deref(), p) {
                        art.insert(art_key.clone(), t);
                    }
                }
                let rows: Vec<TrackRow> = group.iter().map(|t| map_track(*t, art)).collect();
                EphemeralAlbumBlock {
                    group_key: key,
                    disc,
                    sort_title: group_title,
                    title,
                    artist,
                    meta,
                    is_cue: first.cue_file_path.is_some() || first.cue_start_secs.is_some(),
                    art_key,
                    tracks: rows,
                }
            })
            .collect()
    });
    // ORDER BY TITLE, THEN BY DISC NUMBER — and the second half is the fix.
    //
    // The blocks of a box all shared one title, so this comparison tied on
    // every pair and the BTreeMap's insertion order showed through. That key
    // is the STRING "{album}#disc{n}", which sorts lexicographically:
    // #disc1, #disc10, #disc11, #disc12, #disc13, #disc2, #disc3 — exactly the
    // order the owner saw. Comparing the disc as an INTEGER is what fixes it,
    // and it keeps working now that the titles differ.
    // ORDER BY THE ALBUM, THEN BY DISC NUMBER.
    //
    // `sort_title` is the BOX's name, deliberately not the block's displayed
    // title. Sorting on what is shown looks right and is not: once each disc
    // carries its own name, an alphabetical pass puts "Complete Song
    // Collection 1" (disc 11) ahead of "TV Series Soundtrack #01" (disc 1) —
    // measured, that is exactly what a first attempt did. And before the names
    // existed every block tied here, so the BTreeMap's string key showed
    // through as 1, 10, 11, 12, 13, 2, 3. Both failures are the same mistake:
    // ordering discs by anything other than their number.
    albums.sort_by(|a, b| {
        a.sort_title
            .to_lowercase()
            .cmp(&b.sort_title.to_lowercase())
            .then(a.disc.cmp(&b.disc))
    });
    EphemeralDoc {
        name: name.to_string(),
        title,
        folder_context,
        path: path.to_string(),
        track_count: tracks.len(),
        total_duration: crate::local_rows::total_duration(
            tracks.iter().map(|t| t.duration_secs).sum(),
        ),
        multi_album,
        albums,
    }
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// Longest tab label we will hand the segmented bar. `QbzTabBar` sizes a
/// segment to its text (`QbzTabBar.qml:47` implicitWidth), so an unbounded
/// folder name would push the whole bar off screen next to four one-word
/// siblings.
const LABEL_MAX: usize = 24;

/// What the tab and the nav flyout call this session — the name of the THING
/// that is open, never a verb. ("Now Playing" was considered and rejected: it
/// is false the moment a folder sits open while something else plays, and the
/// tab would then contradict the now-playing bar on the same screen.)
///
/// Most specific first: a single-album session is named by its album, because
/// that is the usual case — a folder IS an album, and a disc IS an album. A
/// multi-album folder, or one whose tags gave no title, falls back to the
/// medium's display name, which is exactly what the pane's own header shows.
fn display_label(doc: &EphemeralDoc) -> String {
    let raw = doc.title.as_str();
    if raw.is_empty() {
        return qbz_i18n::t("Media");
    }
    // Count CHARACTERS, not bytes: truncating a UTF-8 string by byte index
    // panics mid-codepoint, and this text is user data — a Japanese album
    // title is three bytes per character.
    if raw.chars().count() > LABEL_MAX {
        let cut: String = raw.chars().take(LABEL_MAX - 1).collect();
        format!("{cut}…")
    } else {
        raw.to_string()
    }
}

/// Holds the `Open` chip busy for exactly as long as this value lives.
///
/// A guard rather than a matched pair of setters, because opening has more
/// exits than it looks like it does: the picker is cancelled, the drive is
/// empty, the image is not a SACD, the task fails. Every one of those has to
/// give the button back, and a `Drop` is the only version of that which
/// cannot be forgotten by the next medium somebody adds.
///
/// It counts rather than flips a bool because an open is a RELAY, not a single
/// task: the reader hands off to `adopt_tracks`, which runs on a task of its
/// own, and the two guards are deliberately alive at the same time so the
/// spinner cannot blink off in the seam between them.
static OPENING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct OpenBusy;

impl OpenBusy {
    pub fn begin() -> Self {
        publish_opening(OPENING.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1);
        Self
    }
}

impl Drop for OpenBusy {
    fn drop(&mut self) {
        publish_opening(OPENING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) - 1);
    }
}

fn publish_opening(outstanding: usize) {
    let busy = outstanding > 0;
    ui(move |mut b| b.as_mut().set_local_disc_opening(busy));
}

fn publish_doc(doc: &EphemeralDoc, loading: bool) {
    publish_doc_with_navigation(doc, loading, false);
}

/// An explicit open navigates only after its session is visible to QML.
/// Restore, scan completion and metadata updates must not steal navigation.
fn publish_doc_with_navigation(doc: &EphemeralDoc, loading: bool, user_open: bool) {
    let json = to_json(doc);
    let label = display_label(doc);
    ui(move |mut b| {
        b.as_mut().set_local_ephemeral_active(true);
        b.as_mut()
            .set_local_ephemeral_json(QString::from(json.as_str()));
        b.as_mut()
            .set_local_ephemeral_label(QString::from(label.as_str()));
        b.as_mut().set_local_ephemeral_loading(loading);
        if user_open {
            // QML handles this signal synchronously and rejects an inactive
            // ephemeral tab. Publish it LAST, in the same Qt-thread callback.
            // A sequence also fires when another session is already active.
            let seq = OPEN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            b.as_mut().set_local_ephemeral_open_seq(seq);
        }
    });
}

/// Back to the closed state (`""` parses to null QML-side, which collapses the
/// pane and restores the normal flat/tree browse).
fn publish_closed() {
    crate::disc_identity::clear();
    ui(|mut b| {
        b.as_mut().set_local_ephemeral_active(false);
        b.as_mut().set_local_ephemeral_loading(false);
        b.as_mut().set_local_ephemeral_json(QString::from(""));
        b.as_mut().set_local_ephemeral_label(QString::from(""));
        b.as_mut().set_local_ephemeral_is_cd(false);
    });
}

/// Persist / forget the folder PATH (the session itself never persists). The
/// same `locallibrary_ui.json` key the Slint frontend writes.
fn save_path(path: Option<&str>) {
    let mut prefs = read_prefs();
    prefs.ephemeral_folder = path.map(|p| p.to_string());
    write_prefs(&prefs);
}

// ---------------------------------------------------------------------------
// Open / clear
// ---------------------------------------------------------------------------

/// Toolbar button: native folder picker -> scan -> pane.
pub fn open() {
    crate::spawn(async move {
        let _busy = OpenBusy::begin();
        let Some(path) = pick_dir(&qbz_i18n::t("Choose a folder to play"), dirs::audio_dir()).await
        else {
            return; // cancelled
        };
        scan(Some(crate::app()), path).await;
    });
}

/// Open a KNOWN path (no picker) — the seam a drag-drop or a CLI argument
/// would use, and what `open()` degrades to on a desktop with no chooser.
pub fn open_path(path: String) {
    if path.is_empty() {
        return;
    }
    crate::spawn(async move {
        scan(Some(crate::app()), path).await;
    });
}

/// Publish a session built from a track list rather than a directory scan — a
/// physical CD or a manually opened SACD image.
///
/// It goes through the SAME `publish_doc` as a folder, so the pane, the tab,
/// the label and the open sequence all behave identically: a disc is not a
/// second kind of session, it is the same session with a different source of
/// tracks.
///
/// This SESSION is not rehydrated. Re-opening a physical drive at boot could
/// find a different CD (or none), so it deliberately starts clean. SACD
/// library indexing is a separate folder-scan concern; the user still
/// explicitly opens an image to create this live pane.
pub fn adopt_tracks(label: &str, tracks: Vec<LocalTrack>) {
    let label = label.to_string();
    // Taken HERE, on the reader's own thread, so it overlaps the reader's
    // guard: taken inside the task instead, the reader could have finished and
    // released the chip before this task was ever scheduled.
    let busy = OpenBusy::begin();
    crate::spawn(async move {
        let _busy = busy;
        wipe_if_playing(&crate::app()).await;
        // Derived from the rows, not passed in: whoever builds the session
        // already knows what the tracks are, and a separate flag is one more
        // thing a future medium can forget to set.
        let is_cd = tracks
            .first()
            .map(|t| qbz_disc::CdRef::is_cd_path(&t.file_path))
            .unwrap_or(false);
        match STATE.open_tracks(&label, tracks) {
            Ok(res) => {
                ui(move |mut b| {
                    b.as_mut().set_local_ephemeral_is_cd(is_cd);
                });
                log::info!(
                    "[qbz-qt] ephemeral adopted {} ({} tracks)",
                    label,
                    res.tracks.len()
                );
                publish_doc_with_navigation(&build_doc(&label, &label, &res.tracks), false, true);
            }
            Err(e) => {
                log::warn!("[qbz-qt] ephemeral adopt failed: {e}");
                publish_closed();
            }
        }
    });
}

/// Rename the OPEN session's rows — the metadata button's landing.
///
/// Deliberately a MUTATION of the live session rather than a re-open: the
/// queue may already hold these ids and a re-open renumbers them from the
/// synthetic floor, which would leave a playing track orphaned mid-song. Same
/// reason `set_session_artwork` uses `replace_tracks_preserving_ids`.
///
/// `album_group_key` is deliberately NOT touched. It is the pane's grouping
/// key, and rewriting it while the album title changes would split one disc
/// into two blocks for exactly one frame — a flicker with no upside, since
/// nothing keys off it but the grouping itself.
///
/// Track naming is positional and defensive: a provider's release can carry a
/// different track count than the disc in the drive (a hidden track, a
/// mixed-mode disc), and pairing by position without checking is how track 5
/// gets track 6's name.
pub fn apply_naming(
    album: &str,
    album_artist: &str,
    year: Option<u32>,
    titles: &[(String, String)],
) {
    let Some(label) = STATE.current_folder_path() else {
        return;
    };
    let mut tracks = STATE.tracks_snapshot();
    if tracks.is_empty() {
        return;
    }
    for (i, t) in tracks.iter_mut().enumerate() {
        if !album.is_empty() {
            t.album = album.to_string();
            t.album_group_title = album.to_string();
        }
        if !album_artist.is_empty() {
            t.album_artist = Some(album_artist.to_string());
            t.artist = album_artist.to_string();
        }
        t.year = year;
        if let Some((title, artist)) = titles.get(i) {
            if !title.is_empty() {
                t.title = title.clone();
            }
            if !artist.is_empty() {
                t.artist = artist.clone();
            }
        }
    }
    if let Err(e) = STATE.replace_tracks_preserving_ids(&tracks) {
        log::warn!("[qbz-qt] ephemeral: naming update failed: {e}");
        return;
    }
    // The LABEL follows the album — it is what the tab, the nav flyout and the
    // pane header all read, and leaving it on the old name would make the
    // correction look like it half-applied.
    let name = if album.is_empty() {
        label.as_str()
    } else {
        album
    };
    log::info!("[qbz-qt] ephemeral: renamed to {name:?}");
    publish_doc(&build_doc(name, name, &tracks), false);
}

/// Attach artwork to the OPEN session after the fact.
///
/// A disc's cover does not come with the disc — it is fetched, and fetching it
/// took 9.4 s on the owner's album because the Cover Art Archive redirects to
/// archive.org. Blocking the session on that meant a click that appeared to do
/// nothing, so the tracks land first and this patches the art in when it
/// arrives.
///
/// A no-op once the user has moved on: if the session was replaced or closed
/// while the download ran, a late cover must not resurrect it or paint itself
/// onto whatever is open now.
pub fn set_session_artwork(path: &str) {
    let path = path.to_string();
    let Some(label) = STATE.current_folder_path() else {
        return;
    };
    let mut tracks = STATE.tracks_snapshot();
    if tracks.is_empty() {
        return;
    }
    for t in tracks.iter_mut() {
        t.artwork_path = Some(path.clone());
    }
    // Re-seat the rows so the ids stay the same — the queue may already hold
    // them, and a re-numbered session would leave a playing track orphaned.
    if let Err(e) = STATE.replace_tracks_preserving_ids(&tracks) {
        log::warn!("[qbz-qt] ephemeral: artwork update failed: {e}");
        return;
    }
    log::info!("[qbz-qt] ephemeral: cover attached to {label}");
    publish_doc(&build_doc(&label, &label, &tracks), false);

    // The store is not the only copy. `local_queue_track` snapshots the row's
    // artwork INTO the queue at enqueue time, and the now-playing bar, the
    // miniplayer, the immersive view and MPRIS all read the QUEUE — so a disc
    // the user started playing before its cover landed stays blank on every
    // one of those surfaces no matter how correct this session is. The url is
    // built the same way `local_queue_track` builds it (a local cover is a
    // `file://`), because that is the form the whole taxonomy downstream
    // expects.
    let ids: Vec<u64> = tracks.iter().map(|t| t.id as u64).collect();
    let url = qbz_models::fs_url::file_url(&path);
    crate::spawn(async move {
        let runtime = crate::app();
        let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
            return;
        };
        if runtime.core().patch_queue_artwork(&ids, &url).await {
            crate::playback_qt::refresh_now_playing(&runtime).await;
        }
    });
}

/// Boot: re-open the persisted folder, if any. Silent — a folder that moved
/// away just clears the stale pref.
pub fn rehydrate() {
    let Some(path) = read_prefs().ephemeral_folder.filter(|p| !p.is_empty()) else {
        return;
    };
    crate::spawn(async move {
        scan(None, path).await;
    });
}

/// Clear button: drop the pane, the in-memory store and the persisted path.
pub fn clear() {
    let runtime = crate::app();
    crate::spawn(async move {
        wipe_if_playing(&runtime).await;
        STATE.clear();
        save_path(None);
        publish_closed();
    });
}

/// The scan body shared by the picker, an explicit path and the boot
/// rehydrate. When a runtime is given (a user-driven open), a currently
/// playing ephemeral track is wiped FIRST — the next session reuses its
/// synthetic ids, so a survivor would false-highlight a row in the new folder.
/// Counts USER-initiated opens. The boot restore does not touch it.
static OPEN_SEQ: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

async fn scan(runtime: Option<Runtime>, path: String) {
    // A folder replaces whatever was open. If that was a disc, the identity
    // has to go with it, or the metadata button would write a correction for
    // a record nobody is holding.
    crate::disc_identity::clear();
    if let Some(rt) = &runtime {
        wipe_if_playing(rt).await;
    }
    let name = folder_display_name(&path);
    // Header + spinner immediately; the scan of a big folder is not instant.
    publish_doc_with_navigation(
        &EphemeralDoc {
            name: name.clone(),
            title: name.clone(),
            folder_context: String::new(),
            path: path.clone(),
            track_count: 0,
            total_duration: String::new(),
            multi_album: false,
            albums: Vec::new(),
        },
        true,
        runtime.is_some(),
    );
    let scan_path = path.clone();
    let result =
        tokio::task::spawn_blocking(move || STATE.open_folder(Path::new(&scan_path))).await;
    match result {
        Ok(Ok(res)) => {
            log::info!(
                "[qbz-qt] ephemeral opened {} ({} tracks, {} skipped)",
                path,
                res.tracks.len(),
                res.skipped_files
            );
            publish_doc(&build_doc(&name, &path, &res.tracks), false);
            save_path(Some(&path));
        }
        Ok(Err(e)) => {
            log::warn!("[qbz-qt] ephemeral open failed: {e}");
            STATE.clear();
            save_path(None);
            publish_closed();
        }
        Err(e) => {
            log::warn!("[qbz-qt] ephemeral scan task failed: {e}");
            publish_closed();
        }
    }
}

/// Stop + drop the queue when what is playing came from the session being
/// replaced or cleared (the Slint's `wipeEphemeralPlaybackArtifacts`).
async fn wipe_if_playing(runtime: &Runtime) {
    let is_eph = runtime
        .core()
        .current_track()
        .await
        .map(|t| is_ephemeral_id(t.id as i64))
        .unwrap_or(false);
    if !is_eph {
        return;
    }
    // Drops everything INCLUDING the current track, stops, republishes the
    // queue and the now-playing chrome.
    crate::queue_qt::clear_queue(runtime).await;
}

/// File chooser for a disc IMAGE. The folder picker's sibling: a FILE
/// selection with an `*.iso` filter, starting in Downloads rather than Music,
/// because that is where a downloaded image lands.
pub(crate) async fn pick_image_blocking() -> Option<String> {
    let start = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));
    let handle = rfd::AsyncFileDialog::new()
        .set_title(&qbz_i18n::t("Choose a disc image"))
        .set_directory(&start)
        // Both cases: a filter is a match on the literal extension, and a
        // `.ISO` off a Windows-written disc is the same file.
        .add_filter(qbz_i18n::t("Disc images"), &["iso", "ISO"])
        .pick_file()
        .await?;
    Some(handle.path().to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Playback (the local seam — synthetic ids resolve from the in-memory store)
// ---------------------------------------------------------------------------

/// Play-all header button: the whole folder becomes the queue, scan order.
pub async fn play_all(runtime: &Runtime, shuffle: bool) {
    let mut rows = tracks_snapshot();
    if shuffle {
        // The same shape `lib::play_album(.., shuffle)` gives an indexed
        // album: the QUEUE is shuffled once and playback starts at its head,
        // rather than leaving the order alone and flipping a player mode.
        // That keeps the queue panel honest — what you see is what plays.
        shuffle_in_place(&mut rows);
    }
    play_rows(runtime, rows, 0).await;
}

/// Fisher-Yates over splitmix64. No `rand` dependency (this crate has none),
/// but NOT a throwaway generator either.
///
/// The first version used a bare xorshift64 seeded from the clock, with a
/// comment claiming a folder shuffle "is not a place that needs a good
/// generator". Measured over eight consecutive shuffles of a ten-track album,
/// one track opened FOUR of them. Shuffle quality is something the listener
/// sees directly, so the comment was wrong and this is the fix.
///
/// splitmix64 finalises each state with two xor-shift-multiplies, which
/// avalanche far better than a raw xorshift — and the modulo below reads the
/// LOW bits, which is exactly where a weak generator is weakest. The counter
/// in the seed is what keeps two shuffles inside the same clock tick from
/// producing the same permutation.
fn shuffle_in_place<T>(v: &mut [T]) {
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x2545_F491_4F6C_DD1D);
    let mut state = nanos
        ^ NONCE
            .fetch_add(0x9E37_79B9_7F4A_7C15, std::sync::atomic::Ordering::Relaxed)
            .rotate_left(32);

    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };

    for i in (1..v.len()).rev() {
        v.swap(i, (next() % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod shuffle_tests {
    use super::*;

    /// Not "is it random" — that is not testable here — but the two failures
    /// that actually shipped: a permutation that loses or duplicates items,
    /// and a first position that is effectively pinned. 2000 shuffles of ten
    /// items should put every item first roughly 200 times; the old xorshift
    /// put one item first 50% of the time in a hand sample.
    #[test]
    fn every_item_survives_and_no_position_is_pinned() {
        let mut firsts = [0usize; 10];
        for _ in 0..2000 {
            let mut v: Vec<usize> = (0..10).collect();
            shuffle_in_place(&mut v);
            let mut seen = v.clone();
            seen.sort_unstable();
            assert_eq!(
                seen,
                (0..10).collect::<Vec<_>>(),
                "shuffle lost or duplicated items"
            );
            firsts[v[0]] += 1;
        }
        // Generous bounds: this guards against a PINNED position, not against
        // a merely mediocre generator. Uniform is 200; anything outside
        // 100..350 means the low bits are not moving.
        for (item, &count) in firsts.iter().enumerate() {
            assert!(
                (100..=350).contains(&count),
                "item {item} opened {count}/2000 shuffles — the distribution is skewed"
            );
        }
    }

    #[test]
    fn the_degenerate_lengths_do_not_panic() {
        let mut empty: Vec<u8> = vec![];
        shuffle_in_place(&mut empty);
        let mut one = vec![7];
        shuffle_in_place(&mut one);
        assert_eq!(one, vec![7]);
    }
}

/// Per-album play (multi-album sessions only): that block becomes the queue.
pub async fn play_album(runtime: &Runtime, group_key: String) {
    play_rows(runtime, album_tracks(&group_key), 0).await;
}

/// Track row click: the track's ALBUM BLOCK becomes the queue, starting there.
pub async fn play_track(runtime: &Runtime, track_id: i64) {
    let Some(track) = get_track(track_id) else {
        log::warn!("[qbz-qt] ephemeral play: track {track_id} is not in the session");
        return;
    };
    let tracks = album_tracks(&album_key_of(&track));
    let start = tracks.iter().position(|t| t.id == track_id).unwrap_or(0);
    play_rows(runtime, tracks, start).await;
}

/// `set_queue` then the AUDIBLE step — the same order `local_playback`'s own
/// queue builders use, with the same `local_queue_track` mapping (so an
/// ephemeral row reaches the queue panel / now-playing bar identical to any
/// other local file).
async fn play_rows(runtime: &Runtime, tracks: Vec<LocalTrack>, start: usize) {
    if tracks.is_empty() {
        return;
    }
    let queue: Vec<QueueTrack> = tracks
        .iter()
        .map(crate::local_playback::local_queue_track)
        .collect();
    let start = start.min(queue.len() - 1);
    // STAMP THE ORIGIN. Without it the song card falls back to the track's
    // `album_id`, which for a disc is the synthetic grouping key
    // (`cdda|||Fear Inoculum`) — not an album any catalogue can open, so the
    // "playing from" glyph led to a page that does not exist. The session is
    // the origin, and the place that shows it is the Open pane.
    let label = STATE
        .current_folder_path()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "session".to_string());
    let queue = crate::playback_qt::stamped(
        queue,
        crate::playback_qt::PlayContext::new("ephemeral", &label),
    );
    if queue.is_empty() {
        return;
    }
    let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
        return;
    };
    let start = start.min(queue.len() - 1);
    let first = queue[start].clone();
    runtime.core().set_queue(queue, Some(start)).await;
    crate::playback_qt::publish_queue(runtime).await;
    // THE shared audible step. This used to call a local `play_file` that was
    // "`local_playback::play_local_file` 1:1 apart from the lookup" — a third
    // copy of the same routine, and the only one whose CUE fast path compared
    // FILE PATHS instead of track ids (i.e. the only one that actually worked).
    // `LocalSource::track_row` reads the session store for an ephemeral id, so
    // the seam resolves these rows with no arm of their own, and `audible_qt`
    // keeps the path comparison for every source.
    if let Err(e) = crate::audible_qt::play_queue_track(runtime, &first).await {
        log::error!(
            "[qbz-qt] ephemeral play: track {} not playable: {e}",
            first.id
        );
    }
    crate::playback_qt::refresh_now_playing(runtime).await;
}

#[cfg(test)]
mod display_label_tests {
    use super::*;

    fn doc(name: &str, albums: Vec<(&str, &str)>, multi: bool) -> EphemeralDoc {
        let title = if !multi && albums.len() == 1 && !albums[0].0.is_empty() {
            albums[0].0.to_string()
        } else {
            name.to_string()
        };
        EphemeralDoc {
            name: name.to_string(),
            title,
            folder_context: String::new(),
            path: "/tmp/x".to_string(),
            track_count: 0,
            total_duration: String::new(),
            multi_album: multi,
            albums: albums
                .into_iter()
                .map(|(title, artist)| EphemeralAlbumBlock {
                    group_key: String::new(),
                    disc: 0,
                    sort_title: title.to_string(),
                    title: title.to_string(),
                    artist: artist.to_string(),
                    meta: String::new(),
                    is_cue: false,
                    art_key: String::new(),
                    tracks: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn one_album_is_named_by_its_album() {
        // The usual case, and the reason the chain starts here: a folder IS
        // an album, and so is a disc.
        let d = doc(
            "Pink Floyd London Live 8 Reunion 2005",
            vec![("Live 8", "Pink Floyd")],
            false,
        );
        assert_eq!(display_label(&d), "Live 8");
    }

    #[test]
    fn several_albums_fall_back_to_the_medium_name() {
        let d = doc("dsdsmoke", vec![("A", "x"), ("B", "y")], true);
        assert_eq!(display_label(&d), "dsdsmoke");
    }

    #[test]
    fn an_untitled_album_falls_back_too() {
        // Tags gave no album title: naming the tab "" would render an empty
        // segment with a lone count badge.
        let d = doc("Disc 1", vec![("", "")], false);
        assert_eq!(display_label(&d), "Disc 1");
    }

    #[test]
    fn a_nameless_session_gets_the_generic_word() {
        assert_eq!(display_label(&doc("", vec![], false)), qbz_i18n::t("Media"));
    }

    #[test]
    fn a_long_name_is_elided_by_characters_not_bytes() {
        // The elision exists because QbzTabBar sizes a segment to its text.
        let long = "Symphony No. 9 in D minor, Op. 125 — Choral";
        let out = display_label(&doc(long, vec![], false));
        assert_eq!(out.chars().count(), LABEL_MAX);
        assert!(out.ends_with('…'));

        // And the byte-vs-char part is not theoretical: slicing this by byte
        // index would panic mid-codepoint.
        let jp = "交響曲第九番ニ短調作品百二十五合唱付きベートーヴェン不滅の傑作";
        assert!(jp.chars().count() > LABEL_MAX);
        let out = display_label(&doc(jp, vec![], false));
        assert_eq!(out.chars().count(), LABEL_MAX);
    }

    #[test]
    fn consistent_collection_title_requires_every_track_to_agree() {
        let track = |album: &str| LocalTrack {
            album: album.to_string(),
            album_group_title: album.to_string(),
            ..Default::default()
        };
        let one_album = vec![track("Sorceress"), track("  Sorceress ")];
        assert_eq!(
            consistent_collection_title(&one_album).as_deref(),
            Some("Sorceress")
        );

        let mix = vec![track("Sorceress"), track("Death Magnetic")];
        assert!(consistent_collection_title(&mix).is_none());
        assert!(consistent_collection_title(&[LocalTrack::default()]).is_none());
    }

    #[test]
    fn a_name_exactly_at_the_cap_is_left_alone() {
        let exact: String = "a".repeat(LABEL_MAX);
        assert_eq!(display_label(&doc(&exact, vec![], false)), exact);
    }
}
