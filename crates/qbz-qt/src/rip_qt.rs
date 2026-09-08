//! "Rip this CD into my library": the Qt side of the job.
//!
//! Everything domain-shaped lives in `qbz-rip` (read, encode, tag, name); this
//! file turns the open session plus the wizard's answers into a plan, runs it,
//! and reports progress. The QUESTIONS live in `rip_wizard_qt` — this file
//! asks nothing.
//!
//! The DESTINATION is always asked, never guessed. A ripper that decides where
//! your music goes is a ripper you have to clean up after, and the app already
//! knows several plausible folders — which is exactly why it must not pick one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use cxx_qt_lib::QString;
use serde::Serialize;

use crate::local_bridge::ui;

// ---------------------------------------------------------------------------
// Live status — what the progress modal reads
// ---------------------------------------------------------------------------

/// The job, as it stands right now.
///
/// SEPARATE from `local_rip_progress` (the one-line "3/7 · 45%" the pane
/// shows) rather than replacing it: the pane's line is a summary that must
/// stay cheap, and this is the whole document a modal needs. One publisher
/// feeds both, so they can never disagree about which track is playing.
#[derive(Serialize, Default, Clone)]
struct Status {
    active: bool,
    /// The PANEL is showing. It rides the same document as the job for the
    /// reason every other modal in this app does: the pane that opens it and
    /// the shell that mounts it are in different subtrees, and a bridge
    /// property is the only channel both can reach. It is also what lets a
    /// finished job close its own panel.
    #[serde(rename = "panelOpen")]
    panel_open: bool,
    album: String,
    destination: String,
    /// 0-based index of the track being read. Everything before it is written,
    /// everything after is waiting.
    index: usize,
    count: usize,
    /// 0..1 within the CURRENT track.
    fraction: f32,
    /// 0..1 across the whole disc.
    overall: f32,
    /// The user asked to stop and the current track is still finishing its
    /// chunk. A latch, not a state: it goes down when the job ends.
    cancelling: bool,
    tracks: Vec<StatusTrack>,
}

#[derive(Serialize, Clone)]
struct StatusTrack {
    number: u32,
    title: String,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
/// Asked to stop. Read by the progress callback, which is the ONLY place that
/// can stop a rip: `qbz_rip::rip` is a blocking loop on a worker thread and
/// its callback's `false` is the documented way out (`RipError::Cancelled`).
static CANCEL: AtomicBool = AtomicBool::new(false);

fn with_status<R>(f: impl FnOnce(&mut Status) -> R) -> R {
    let mut guard = STATUS.lock().unwrap();
    f(guard.get_or_insert_with(Status::default))
}

fn publish_status() {
    let json = with_status(|s| serde_json::to_string(s).unwrap_or_else(|_| "null".into()));
    ui(move |mut b| {
        b.as_mut()
            .set_local_rip_status(QString::from(json.as_str()))
    });
}

/// Show / hide the progress panel. The JOB is untouched either way — this
/// closes a window, never a rip.
pub fn set_panel_open(open: bool) {
    with_status(|s| s.panel_open = open);
    publish_status();
}

/// Stop after the current chunk.
///
/// It does NOT delete anything, and the modal says so. Whatever was already
/// written is a complete, playable FLAC file; throwing away work the user
/// waited minutes for because they stopped the NEXT track would be a worse
/// surprise than a folder with four tracks in it.
pub fn cancel() {
    if !with_status(|s| s.active) {
        return;
    }
    CANCEL.store(true, Ordering::SeqCst);
    with_status(|s| s.cancelling = true);
    publish_status();
    log::info!("[qbz-qt] rip: cancel requested");
}

/// Run a rip that the WIZARD has already specified.
///
/// The destination, the naming and the follow-up all arrive decided — this
/// file no longer asks anything. That is the split the wizard bought: the
/// questions live in one place with the copy that explains them, and this one
/// owns the job.
#[allow(clippy::too_many_arguments)]
pub fn run(
    destination: std::path::PathBuf,
    album: String,
    album_artist: String,
    year: Option<u32>,
    named: Vec<(String, String)>,
    // `selected` is per-DISC-ROW, parallel to `named`: which tracks to write.
    // A partial rip is the ordinary case for a compilation, and the
    // alternative — rip everything and delete the rest — wastes minutes of
    // drive time.
    selected: Vec<bool>,
    library_action: String,
) {
    if !crate::local_ephemeral::session_is_cd() {
        return;
    }
    crate::spawn(async move {
        let Some(plan) = build_plan(
            destination.clone(),
            &album,
            &album_artist,
            year,
            &named,
            &selected,
        ) else {
            crate::toast_qt::error(qbz_i18n::t("Nothing to rip."));
            return;
        };
        let count = plan.tracks.len();
        let album_name = plan.album.clone();
        let folder_id = crate::rip_wizard_qt::library_folder_id();

        CANCEL.store(false, Ordering::SeqCst);
        with_status(|s| {
            let panel_open = s.panel_open;
            *s = Status {
                active: true,
                panel_open,
                album: plan.album.clone(),
                destination: plan.destination.to_string_lossy().into_owned(),
                index: 0,
                count,
                fraction: 0.0,
                overall: 0.0,
                cancelling: false,
                // The PLAN's tracks, not the disc's: a partial rip must not
                // show four rows of "waiting" that will never be written.
                tracks: plan
                    .tracks
                    .iter()
                    .map(|t| StatusTrack {
                        number: t.number,
                        title: t.title.clone(),
                    })
                    .collect(),
            };
        });
        publish_status();

        ui(|mut b| {
            b.as_mut().set_local_rip_active(true);
            b.as_mut().set_local_rip_progress(QString::from("0%"));
        });

        let outcome = tokio::task::spawn_blocking(move || {
            // The callback fires per CHUNK — hundreds of times per track. Both
            // documents are rate-limited to ~10 Hz, with a track boundary
            // always let through: a progress bar does not need 200 updates a
            // second, and a Qt-thread hop per chunk is a queue nobody drains.
            let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
            let mut last_index = usize::MAX;
            qbz_rip::rip(&plan, move |p| {
                // The bar is per-DISC, not per-track: a listener watching a
                // 79-minute album wants to know how far the album is, and a
                // per-track bar that resets seven times reads like a stall.
                // The modal shows BOTH, which is why the status carries the
                // per-track fraction alongside it.
                let overall = (p.track_index as f32 + p.fraction) / p.track_count as f32;
                if CANCEL.load(Ordering::SeqCst) {
                    return false;
                }
                let boundary = p.track_index != last_index;
                if !boundary && last.elapsed() < std::time::Duration::from_millis(100) {
                    return true;
                }
                last = std::time::Instant::now();
                last_index = p.track_index;

                with_status(|s| {
                    s.index = p.track_index;
                    s.fraction = p.fraction;
                    s.overall = overall;
                });
                publish_status();

                let text = format!(
                    "{}/{} · {}%",
                    p.track_index + 1,
                    p.track_count,
                    (overall * 100.0) as u32
                );
                ui(move |mut b| {
                    b.as_mut()
                        .set_local_rip_progress(QString::from(text.as_str()));
                });
                true
            })
        })
        .await;

        let stopped = CANCEL.swap(false, Ordering::SeqCst);
        with_status(|s| {
            s.active = false;
            s.cancelling = false;
            // A cancelled job did not reach the end, and saying 100% would be
            // the one lie a progress bar must never tell.
            if !stopped {
                s.overall = 1.0;
            }
        });
        publish_status();

        ui(|mut b| {
            b.as_mut().set_local_rip_active(false);
            b.as_mut().set_local_rip_progress(QString::default());
        });

        match outcome {
            Ok(Ok(files)) => {
                log::info!("[qbz-qt] rip: {} files written", files.len());
                crate::toast_qt::success(qbz_i18n::tf(
                    "Ripped {} track to your library",
                    "Ripped {} tracks to your library",
                    count as i64,
                    &[&count.to_string()],
                ));
                // The follow-up the wizard promised. It runs ONLY on success:
                // scanning a folder for an album that failed to write is a
                // minute of disk for nothing, and it would report "added".
                crate::rip_wizard_qt::after_rip(&library_action, &destination, folder_id).await;
                log::info!("[qbz-qt] rip: {album_name:?} done");
            }
            Ok(Err(qbz_rip::RipError::Cancelled)) => {
                log::info!("[qbz-qt] rip cancelled by the user");
                crate::toast_qt::success(qbz_i18n::t(
                    "Rip stopped. The files already written are still there.",
                ));
            }
            Ok(Err(e)) => {
                log::warn!("[qbz-qt] rip failed: {e}");
                crate::toast_qt::error(format!("{e}"));
            }
            Err(e) => log::warn!("[qbz-qt] rip task failed: {e}"),
        }
    });
}

/// Turn the open session into a plan, with the WIZARD's naming laid over it.
///
/// The session rows are the source of the disc GEOMETRY — the device and the
/// sector range are read from them and never from the form, because those are
/// facts about the disc rather than opinions about it. Everything a human can
/// be wrong about comes from the form.
///
/// Returns `None` when the session holds no track this can read.
fn build_plan(
    destination: std::path::PathBuf,
    album: &str,
    album_artist: &str,
    year: Option<u32>,
    named: &[(String, String)],
    selected: &[bool],
) -> Option<qbz_rip::RipPlan> {
    let rows = crate::local_ephemeral::tracks_snapshot();
    if rows.is_empty() {
        return None;
    }
    let album = if album.is_empty() {
        rows[0].album.clone()
    } else {
        album.to_string()
    };
    let album_artist = if album_artist.is_empty() {
        rows[0]
            .album_artist
            .clone()
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| rows[0].artist.clone())
    } else {
        album_artist.to_string()
    };

    let tracks: Vec<qbz_rip::RipTrack> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            // Absent means selected: a caller that does not know about partial
            // rips must not silently rip nothing.
            if !selected.get(i).copied().unwrap_or(true) {
                return None;
            }
            let r = qbz_disc::CdRef::parse(&t.file_path)?;
            let (title, artist) = named.get(i).cloned().unwrap_or_default();
            Some(qbz_rip::RipTrack {
                number: t.track_number.unwrap_or(i as u32 + 1),
                title: if title.is_empty() {
                    t.title.clone()
                } else {
                    title
                },
                artist: match artist {
                    a if !a.is_empty() => a,
                    _ if t.artist.is_empty() => album_artist.clone(),
                    _ => t.artist.clone(),
                },
                source: qbz_rip::RipSource::Cd {
                    device: r.device,
                    start_lsn: r.start_lsn,
                    sectors: r.sectors,
                },
            })
        })
        .collect();

    // The provenance the log needs. It comes from the DISC, not from the form:
    // the identity of the record in the drive is not something a user typed.
    let identity = crate::disc_identity::current();
    // The artwork the session is carrying — already a real file on disk (the
    // artwork cache), which `copy_cover` drops in as `cover.jpg`.
    let cover = rows
        .iter()
        .find_map(|t| t.artwork_path.clone())
        .filter(|p| std::path::Path::new(p).is_file())
        .map(std::path::PathBuf::from);

    (!tracks.is_empty()).then(|| qbz_rip::RipPlan {
        destination,
        album,
        album_artist,
        year: year.or(rows[0].year),
        tracks,
        disc_id: identity.as_ref().and_then(|i| i.disc_id.clone()),
        toc_fingerprint: identity.as_ref().map(|i| i.fingerprint.clone()),
        // The DISC's count, not the plan's: that difference is what makes a
        // partial rip legible in the log.
        disc_track_count: rows.len(),
        cover,
    })
}
