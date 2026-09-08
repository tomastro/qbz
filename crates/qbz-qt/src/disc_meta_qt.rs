//! "This is the wrong record" — looking a disc up by hand.
//!
//! The automatic naming is one guess from one provider. It is wrong often
//! enough to need an escape hatch: a CD-DA carries no titles at all, a DiscID
//! names the GEOMETRY rather than the pressing (the owner's *Fear Inoculum*
//! answers with four releases that share a table of contents), and a SACD's
//! Master TOC can carry a spelling nobody likes. This is that escape hatch,
//! for BOTH media.
//!
//! What makes it more than a toy is [`qbz_disc::store`]: an applied result is
//! written as a USER row, and no later automatic lookup may overwrite one. Fix
//! a disc once and it stays fixed across ejects, restarts and re-inserts.
//!
//! The provider work is `qbz_integrations::remote_metadata` (ADR-006) — this
//! file owns the document, the latches and the landing, and nothing about how
//! MusicBrainz or Discogs are spoken to.

use std::sync::Mutex;

use cxx_qt_lib::QString;
use qbz_integrations::remote_metadata::{
    self as rm, RemoteAlbumMetadata, RemoteProvider, RemoteSearchRequest,
};
use serde::Serialize;

use crate::disc_meta_bridge::ui;

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[derive(Serialize, Default, Clone)]
struct Doc {
    open: bool,
    /// "cd" | "sacd" — the modal says which medium it is correcting, because
    /// the two reach it from different places and a disc image is easy to
    /// mistake for the disc in the drive.
    kind: String,
    provider: String,
    query: String,
    searching: bool,
    /// The provider id currently being fetched in full, "" when none. A
    /// per-row latch rather than one global `loading`, so the row the user
    /// clicked is the row that shows a spinner.
    #[serde(rename = "loadingId")]
    loading_id: String,
    applying: bool,
    /// A search has RUN. Without it an empty list on first open reads as "this
    /// disc is not in MusicBrainz", which is a different and discouraging
    /// claim.
    searched: bool,
    #[serde(rename = "rateLimited")]
    rate_limited: bool,
    results: Vec<Row>,
    #[serde(rename = "selectedId")]
    selected_id: String,
    preview: Option<Preview>,
    /// How many tracks the DISC has. The modal shows it next to a candidate's
    /// own count, because a mismatch is the single best signal that a
    /// plausible-looking result is the wrong pressing.
    #[serde(rename = "discTrackCount")]
    disc_track_count: usize,
    /// This disc already carries a correction — the modal offers to drop it.
    #[serde(rename = "hasCorrection")]
    has_correction: bool,
}

#[derive(Serialize, Clone)]
struct Row {
    id: String,
    title: String,
    artist: String,
    year: String,
    #[serde(rename = "trackCount")]
    track_count: i32,
    country: String,
    label: String,
    #[serde(rename = "catalogNumber")]
    catalog_number: String,
    format: String,
}

#[derive(Serialize, Clone)]
struct Preview {
    title: String,
    artist: String,
    year: String,
    tracks: Vec<PreviewTrack>,
}

#[derive(Serialize, Clone)]
struct PreviewTrack {
    number: u32,
    title: String,
}

static STATE: Mutex<Option<Doc>> = Mutex::new(None);
/// The full metadata behind `preview`, kept out of the document because QML
/// never needs the label, barcode or source url — only `apply` does.
static PENDING: Mutex<Option<RemoteAlbumMetadata>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Doc) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    let doc = guard.get_or_insert_with(Doc::default);
    f(doc)
}

fn publish() {
    let json = with(|d| serde_json::to_string(d).unwrap_or_else(|_| "null".into()));
    ui(move |mut b| b.as_mut().set_meta_json(QString::from(json.as_str())));
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

/// Open the modal, seeded from the session that is actually on screen.
///
/// A no-op with no disc open: the button that reaches here is only drawn for a
/// disc, and opening a "correct this record" modal with no record is a dialog
/// that can only be cancelled.
pub fn open() {
    let Some(identity) = crate::disc_identity::current() else {
        // SAY SO. The first version of this function logged nothing on either
        // arm, so a button that opened nothing and a button that was never
        // wired looked identical from the outside — the exact silent
        // degradation the CD lookup was already burned by.
        log::warn!("[qbz-qt] disc meta: open requested with no disc identity");
        crate::toast_qt::error(qbz_i18n::t("No disc is open."));
        return;
    };
    let tracks = crate::local_ephemeral::tracks_snapshot();
    let album = tracks.first().map(|t| t.album.clone()).unwrap_or_default();
    let artist = tracks
        .first()
        .and_then(|t| t.album_artist.clone().filter(|a| !a.is_empty()))
        .or_else(|| tracks.first().map(|t| t.artist.clone()))
        .unwrap_or_default();
    let remembered = qbz_disc::store::get(&identity.fingerprint);

    with(|d| {
        *d = Doc {
            open: true,
            kind: identity.kind.as_str().to_string(),
            // MusicBrainz first: it is the one that knows a DiscID, it needs
            // no proxy, and for a CD it is what named the disc in the first
            // place.
            provider: "musicbrainz".into(),
            // The query is what the user would type anyway. Seeding it beats
            // an empty box that makes them retype what is on screen.
            query: format!("{artist} {album}").trim().to_string(),
            disc_track_count: tracks.len(),
            has_correction: remembered.map(|m| m.edited).unwrap_or(false),
            ..Doc::default()
        };
    });
    log::info!(
        "[qbz-qt] disc meta: open for {} {} ({} tracks)",
        identity.kind.as_str(),
        identity.fingerprint,
        tracks.len()
    );
    publish();
}

pub fn close() {
    with(|d| {
        d.open = false;
        d.results.clear();
        d.preview = None;
        d.selected_id.clear();
        d.searched = false;
    });
    *PENDING.lock().unwrap() = None;
    publish();
}

/// Switch provider. The results go with it — showing MusicBrainz rows under a
/// Discogs heading is how a user applies the wrong one.
pub fn set_provider(word: &str) {
    let Ok(provider) = word.parse::<RemoteProvider>() else {
        log::warn!("[qbz-qt] disc meta: unknown provider {word:?}");
        return;
    };
    with(|d| {
        d.provider = provider_word(provider).to_string();
        d.results.clear();
        d.preview = None;
        d.selected_id.clear();
        d.searched = false;
        d.rate_limited = false;
    });
    *PENDING.lock().unwrap() = None;
    publish();
}

fn provider_word(p: RemoteProvider) -> &'static str {
    match p {
        RemoteProvider::MusicBrainz => "musicbrainz",
        RemoteProvider::Discogs => "discogs",
    }
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

pub fn search(query: &str) {
    let query = query.trim().to_string();
    if query.is_empty() {
        return;
    }
    let (provider, artist) = with(|d| {
        d.query = query.clone();
        d.searching = true;
        d.searched = true;
        d.rate_limited = false;
        d.results.clear();
        d.preview = None;
        d.selected_id.clear();
        (
            d.provider
                .parse::<RemoteProvider>()
                .unwrap_or(RemoteProvider::MusicBrainz),
            // The album artist as the app currently knows it, which is what
            // lets the orchestration split "artist album" back apart.
            crate::local_ephemeral::tracks_snapshot()
                .first()
                .and_then(|t| t.album_artist.clone())
                .unwrap_or_default(),
        )
    });
    publish();

    crate::spawn(async move {
        let response = rm::search(&RemoteSearchRequest {
            provider,
            query,
            catalog_id: None,
            artist: Some(artist).filter(|a| !a.is_empty()),
            limit: Some(20),
        })
        .await;
        log::info!(
            "[qbz-qt] disc meta: {} returned {} result(s){}",
            provider_word(provider),
            response.results.len(),
            if response.rate_limited {
                " (rate limited)"
            } else {
                ""
            }
        );
        with(|d| {
            // A late answer from a provider the user has since switched away
            // from must not repopulate the list.
            if d.provider != provider_word(provider) {
                return;
            }
            d.searching = false;
            d.rate_limited = response.rate_limited;
            d.results = response
                .results
                .iter()
                .map(|r| Row {
                    id: r.provider_id.clone(),
                    title: r.title.clone(),
                    artist: r.artist.clone(),
                    year: r.year.map(|y| y.to_string()).unwrap_or_default(),
                    track_count: r.track_count.map(|c| c as i32).unwrap_or(-1),
                    country: r.country.clone().unwrap_or_default(),
                    label: r.label.clone().unwrap_or_default(),
                    catalog_number: r.catalog_number.clone().unwrap_or_default(),
                    format: r.format.clone().unwrap_or_default(),
                })
                .collect();
        });
        publish();
    });
}

// ---------------------------------------------------------------------------
// Preview one candidate
// ---------------------------------------------------------------------------

/// Fetch a candidate in full and show its track list.
///
/// Selecting is deliberately NOT applying: the track list is the only thing
/// that tells a user whether a plausible title is the right pressing, and
/// making them commit before they can see it is the whole failure this feature
/// exists to fix.
pub fn select(provider_id: &str) {
    let id = provider_id.to_string();
    if id.is_empty() {
        return;
    }
    let provider = with(|d| {
        d.selected_id = id.clone();
        d.loading_id = id.clone();
        d.preview = None;
        d.provider
            .parse::<RemoteProvider>()
            .unwrap_or(RemoteProvider::MusicBrainz)
    });
    publish();

    crate::spawn(async move {
        let fetched = rm::get_album(provider, &id).await;
        match fetched {
            Ok(meta) => {
                let preview = Preview {
                    title: meta.title.clone(),
                    artist: meta.artist.clone(),
                    year: meta.year.map(|y| y.to_string()).unwrap_or_default(),
                    tracks: meta
                        .tracks
                        .iter()
                        .map(|t| PreviewTrack {
                            number: t.track_number as u32,
                            title: t.title.clone(),
                        })
                        .collect(),
                };
                *PENDING.lock().unwrap() = Some(meta);
                with(|d| {
                    if d.selected_id != id {
                        return; // the user moved on
                    }
                    d.loading_id.clear();
                    d.preview = Some(preview);
                });
            }
            Err(e) => {
                log::warn!("[qbz-qt] disc meta: fetch failed: {e}");
                crate::toast_qt::error(qbz_i18n::t("Couldn't load that release."));
                with(|d| d.loading_id.clear());
            }
        }
        publish();
    });
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

/// Write the previewed release onto the open session AND into the store.
///
/// Both, in that order, and neither is optional: the session is what the user
/// sees right now, and the store is what makes the correction outlive the
/// eject.
pub fn apply() {
    let Some(meta) = PENDING.lock().unwrap().clone() else {
        return;
    };
    let Some(identity) = crate::disc_identity::current() else {
        crate::toast_qt::error(qbz_i18n::t("No disc is open."));
        return;
    };
    with(|d| d.applying = true);
    publish();

    let disc_tracks = crate::local_ephemeral::tracks_snapshot().len();
    // Positional pairing, defensive: a release with a different track count
    // than the disc is exactly the case a user is trying to fix, and pairing
    // past the end would name track 5 with track 6's title.
    let named: Vec<(String, String)> = (0..disc_tracks)
        .map(|i| {
            meta.tracks
                .get(i)
                .map(|t| (t.title.clone(), String::new()))
                .unwrap_or_default()
        })
        .collect();
    let year = meta.year.map(|y| y as u32);

    crate::local_ephemeral::apply_naming(&meta.title, &meta.artist, year, &named);

    let memory = qbz_disc::store::DiscMemory {
        album: meta.title.clone(),
        album_artist: meta.artist.clone(),
        year,
        tracks: (0..disc_tracks)
            .map(|i| qbz_disc::store::TrackMemory {
                number: i as u32 + 1,
                title: meta
                    .tracks
                    .get(i)
                    .map(|t| t.title.clone())
                    .unwrap_or_default(),
                artist: meta.artist.clone(),
            })
            .collect(),
        release_id: None,
        release_group_id: None,
        cover_path: None,
        edited: true,
    };
    qbz_disc::store::put_user(&identity.fingerprint, identity.disc_id.as_deref(), &memory);
    log::info!(
        "[qbz-qt] disc meta: applied {:?} by {:?} from {}",
        meta.title,
        meta.artist,
        provider_word(meta.provider)
    );
    crate::toast_qt::success(qbz_i18n::t("Disc details updated"));

    // A different pressing usually means a different cover. MusicBrainz ids
    // are the Cover Art Archive's keys, so the art can be re-resolved for
    // free; Discogs ids are not, and a wrong cover is worse than the old one,
    // so that arm deliberately leaves the artwork alone.
    if matches!(meta.provider, RemoteProvider::MusicBrainz) {
        let release = meta.provider_id.clone();
        let fingerprint = identity.fingerprint.clone();
        crate::spawn(async move {
            if let Some(art) = crate::cdda_qt::fetch_cover_for(&release, None).await {
                qbz_disc::store::set_cover(&fingerprint, &art);
                crate::local_ephemeral::set_session_artwork(&art);
            }
        });
    }

    close();
}

/// Drop this disc's correction and go back to what the lookup (or the disc
/// itself) says. The escape hatch for a correction that turned out wrong —
/// without it a bad apply is permanent, which would make people afraid to use
/// the button at all.
pub fn forget() {
    let Some(identity) = crate::disc_identity::current() else {
        return;
    };
    qbz_disc::store::forget(&identity.fingerprint);
    log::info!("[qbz-qt] disc meta: correction dropped");
    crate::toast_qt::success(qbz_i18n::t(
        "Correction removed. Re-open the disc to look it up again.",
    ));
    with(|d| d.has_correction = false);
    publish();
}
