//! The rip wizard: say what is about to happen, let it be corrected, then do
//! it.
//!
//! The first version asked for a folder and started writing. That is fine
//! until the disc was named wrong — and a CD-DA carries no titles at all, so
//! "named wrong" is the ordinary case, not the edge one. Once files are on
//! disk a bad title is a rename job, so the only cheap moment to fix it is
//! before the first byte is written.
//!
//! FOUR THINGS THE WIZARD OWES THE USER, and they are why it exists:
//!
//!  1. **What it will do.** Which disc, how many tracks, where they go, and
//!     that the output is FLAC — the only format this app writes. A ripper
//!     that silently picks a format is one you find out about later.
//!  2. **The tracks**, with their titles, editable.
//!  3. **A destination**, asked and never guessed.
//!  4. **What happens AFTERWARDS.** A ripped album that the library cannot see
//!     is a folder full of files, so the wizard checks whether the destination
//!     is already inside a registered Local Library folder and offers the
//!     right follow-up: re-scan that folder, or add this one.
//!
//! Edits made here are also written to [`qbz_disc::store`] as a USER row, so
//! they survive the eject and seed the next rip of the same disc.

use std::sync::Mutex;

use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};

use crate::local_bridge::ui;

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[derive(Serialize, Default, Clone)]
struct Doc {
    open: bool,
    /// Seeds only. Once the wizard is open the FORM owns these — Rust must not
    /// republish over what the user is typing, which is why every later
    /// publish carries the same seeds and the QML re-seeds only on open.
    album: String,
    #[serde(rename = "albumArtist")]
    album_artist: String,
    year: String,
    destination: String,
    /// The picker is on a worker; the button says so.
    #[serde(rename = "picking")]
    picking: bool,
    tracks: Vec<Track>,
    /// Where the destination stands relative to the indexed library:
    /// "unknown" (nothing picked yet) · "inside" · "outside".
    #[serde(rename = "libraryState")]
    library_state: String,
    /// The REGISTERED ancestor when `inside` — the folder a re-scan would
    /// walk. Named in the question, because "re-scan your library" and
    /// "re-scan ~/Music" are very different offers.
    #[serde(rename = "libraryFolder")]
    library_folder: String,
    #[serde(rename = "libraryFolderId")]
    library_folder_id: i64,
}

#[derive(Serialize, Clone)]
struct Track {
    number: u32,
    title: String,
    artist: String,
    duration: String,
    /// Seeded TRUE. The user goes from all to fewer, never the other way — a
    /// wizard that starts with nothing ticked makes the common case (rip the
    /// whole disc) the one that takes the most clicks.
    selected: bool,
}

/// What the form sends back. Everything the user could have touched, so the
/// wizard never has to guess which fields are still the seeds.
#[derive(Deserialize, Default)]
struct Edits {
    #[serde(default)]
    album: String,
    #[serde(default, rename = "albumArtist")]
    album_artist: String,
    #[serde(default)]
    year: String,
    #[serde(default)]
    destination: String,
    #[serde(default)]
    tracks: Vec<EditTrack>,
    /// "none" | "rescan" | "add" — what to do with Local Library afterwards.
    #[serde(default)]
    library: String,
}

#[derive(Deserialize, Default, Clone)]
struct EditTrack {
    #[serde(default)]
    number: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    /// Defaults to TRUE when the field is missing: an older form, or a caller
    /// that does not know about partial rips, must not silently rip nothing.
    #[serde(default = "yes")]
    selected: bool,
}

fn yes() -> bool {
    true
}

static STATE: Mutex<Option<Doc>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Doc) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    f(guard.get_or_insert_with(Doc::default))
}

fn publish() {
    let json = with(|d| serde_json::to_string(d).unwrap_or_else(|_| "null".into()));
    ui(move |mut b| b.as_mut().set_local_rip_plan(QString::from(json.as_str())));
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

pub fn open() {
    if !crate::local_ephemeral::session_is_cd() {
        crate::toast_qt::error(qbz_i18n::t("Only a physical CD can be ripped."));
        return;
    }
    let rows = crate::local_ephemeral::tracks_snapshot();
    if rows.is_empty() {
        return;
    }
    let album = rows[0].album.clone();
    let album_artist = rows[0]
        .album_artist
        .clone()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| rows[0].artist.clone());

    with(|d| {
        *d = Doc {
            open: true,
            album: album.clone(),
            album_artist: album_artist.clone(),
            year: rows[0].year.map(|y| y.to_string()).unwrap_or_default(),
            // Deliberately empty. The destination is asked, never guessed —
            // the app knows several plausible music folders, which is exactly
            // why it must not choose one (see `rip_qt`'s header).
            destination: String::new(),
            library_state: "unknown".into(),
            tracks: rows
                .iter()
                .enumerate()
                .map(|(i, t)| Track {
                    number: t.track_number.unwrap_or(i as u32 + 1),
                    title: t.title.clone(),
                    artist: if t.artist.is_empty() {
                        album_artist.clone()
                    } else {
                        t.artist.clone()
                    },
                    duration: crate::local_rows::mmss(t.duration_secs),
                    selected: true,
                })
                .collect(),
            ..Doc::default()
        };
    });
    publish();
}

pub fn close() {
    with(|d| d.open = false);
    publish();
}

// ---------------------------------------------------------------------------
// Destination
// ---------------------------------------------------------------------------

pub fn pick_destination() {
    with(|d| d.picking = true);
    publish();
    crate::spawn(async move {
        let Some(dest) = crate::local_ephemeral::pick_folder_for_rip().await else {
            with(|d| d.picking = false);
            publish();
            return;
        };
        // The library question is answered on a worker: it opens the library
        // db and canonicalizes paths, and neither belongs on the Qt thread.
        let verdict = tokio::task::spawn_blocking({
            let dest = dest.clone();
            move || library_verdict(&dest)
        })
        .await
        .unwrap_or_default();

        with(|d| {
            d.picking = false;
            d.destination = dest;
            d.library_state = verdict.state;
            d.library_folder = verdict.folder;
            d.library_folder_id = verdict.folder_id;
        });
        publish();
    });
}

#[derive(Default)]
struct Verdict {
    state: String,
    folder: String,
    folder_id: i64,
}

/// Is this destination already covered by an indexed folder?
///
/// Canonicalized on BOTH sides, because `add_folder` canonicalizes before it
/// inserts: comparing a symlinked `~/Music` against its resolved row would
/// answer "outside" for a folder that is very much inside, and the user would
/// be offered a duplicate registration.
///
/// The ancestor test is component-wise, not a string prefix: `/music-backup`
/// starts with `/music` and is not inside it.
fn library_verdict(destination: &str) -> Verdict {
    let dest = std::fs::canonicalize(destination)
        .unwrap_or_else(|_| std::path::PathBuf::from(destination));
    let folders = crate::library_db_qt::with_db(false, |db| Ok(db.get_folders_with_metadata()?))
        .unwrap_or_default();
    for f in &folders {
        let root = std::path::Path::new(&f.path);
        if dest == root || dest.starts_with(root) {
            return Verdict {
                state: "inside".into(),
                folder: f.path.clone(),
                folder_id: f.id,
            };
        }
    }
    Verdict {
        state: "outside".into(),
        folder: String::new(),
        folder_id: 0,
    }
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

/// Run the rip with the form's values.
///
/// The edits land in THREE places and all three are the point: the plan that
/// is about to be written, the open session (so the pane stops showing the old
/// titles the moment you correct them), and the disc store (so the next rip of
/// this disc starts from the corrected names).
pub fn start(edits_json: &str) {
    let edits: Edits = serde_json::from_str(edits_json).unwrap_or_default();
    if edits.destination.trim().is_empty() {
        crate::toast_qt::error(qbz_i18n::t("Choose where the album should go first."));
        return;
    }
    let album = edits.album.trim().to_string();
    let album_artist = edits.album_artist.trim().to_string();
    let year = edits.year.trim().parse::<u32>().ok();

    // The session and the store learn the corrections whether or not the rip
    // itself succeeds: the user fixed the names, and a failed write must not
    // throw that away.
    let named: Vec<(String, String)> = edits
        .tracks
        .iter()
        .map(|t| (t.title.trim().to_string(), t.artist.trim().to_string()))
        .collect();
    let selected: Vec<bool> = edits.tracks.iter().map(|t| t.selected).collect();
    if !selected.iter().any(|s| *s) {
        crate::toast_qt::error(qbz_i18n::t("Pick at least one track to rip."));
        return;
    }

    crate::local_ephemeral::apply_naming(&album, &album_artist, year, &named);
    if let Some(identity) = crate::disc_identity::current() {
        qbz_disc::store::put_user(
            &identity.fingerprint,
            identity.disc_id.as_deref(),
            &qbz_disc::store::DiscMemory {
                album: album.clone(),
                album_artist: album_artist.clone(),
                year,
                tracks: edits
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| qbz_disc::store::TrackMemory {
                        number: if t.number > 0 { t.number } else { i as u32 + 1 },
                        title: t.title.trim().to_string(),
                        artist: t.artist.trim().to_string(),
                    })
                    .collect(),
                release_id: None,
                release_group_id: None,
                cover_path: None,
                edited: true,
            },
        );
    }

    close();
    crate::rip_qt::run(
        std::path::PathBuf::from(edits.destination.trim()),
        album,
        album_artist,
        year,
        named,
        selected,
        edits.library,
    );
}

/// The registered folder the destination sits in, or 0. Read at rip time
/// rather than passed through the form, because it is the WIZARD's finding
/// about the filesystem and not something a user typed.
pub fn library_folder_id() -> i64 {
    with(|d| d.library_folder_id)
}

/// The follow-up the wizard promised, after the files are on disk.
///
/// Ordering matters and is not arbitrary: a folder has to be REGISTERED before
/// a scan of it can find anything, and a scan of one folder id beats a scan of
/// `None`, which re-walks the entire library (minutes, on a large one) to
/// discover an album we can point at directly.
pub async fn after_rip(action: &str, destination: &std::path::Path, folder_id: i64) {
    match action {
        "rescan" => {
            if folder_id > 0 {
                log::info!("[qbz-qt] rip: re-scanning library folder {folder_id}");
                crate::settings_qt::library::scan(Some(folder_id));
            }
        }
        "add" => {
            let path = destination.to_string_lossy().into_owned();
            log::info!("[qbz-qt] rip: adding {path} to the library");
            crate::settings_qt::library::add_folder(path.clone()).await;
            // `add_folder` reports its own status and does not hand back the
            // id, so the row is looked up — a silent no-op must not be
            // followed by a scan that reports success it never had.
            let added = tokio::task::spawn_blocking(move || {
                let canonical = std::fs::canonicalize(&path)
                    .map(|c| c.to_string_lossy().into_owned())
                    .unwrap_or(path);
                crate::library_db_qt::with_db(false, |db| Ok(db.get_folders_with_metadata()?))
                    .and_then(|f| f.iter().find(|f| f.path == canonical).map(|f| f.id))
            })
            .await
            .ok()
            .flatten();
            match added {
                Some(id) => {
                    crate::settings_qt::library::scan(Some(id));
                }
                None => {
                    crate::toast_qt::error(qbz_i18n::t("Couldn't add that folder to the library"))
                }
            }
        }
        _ => {}
    }
}
