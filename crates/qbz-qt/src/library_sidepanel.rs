//! Library > Artists **sidepanel** — the two-column mode of the Artists tab
//! (`FavoritesView.slint:2014-2185`): the A-Z grouped favourite artists on the
//! left, the SELECTED artist's releases on the right.
//!
//! Reference: `main.rs::FavoritesActions::on_select_artist` (:22298) +
//! `favorites::apply_selected_artist` (`favorites.rs:846`). Both reuse the
//! standalone artist page's `/artist/page` `release_type` classifier rather
//! than re-deriving sections, "per the user's decision, so the sections are
//! server-authoritative (Discography / EPs & Singles / Live / Compilations /
//! Others)". This does the same: `artist_qt::load_artist` is the port of that
//! very loader, so the sidepanel and the full artist page can never disagree
//! about what an artist released.
//!
//! The LEFT column is pure QML — it groups the artist rows the Library feed
//! already carries, exactly like the grid's own A-Z grouping.
//!
//! Artwork: the published rows carry `artUrl` only, and QML resolves covers
//! through the SAME url-keyed pair the artist page uses
//! (`QbzShell.sidebarArtworkWindow` -> `QbzLibrary.libraryArtworkReady`). No
//! second artwork mechanism.

use std::sync::atomic::{AtomicU64, Ordering};

use cxx_qt_lib::QString;
use serde::Serialize;

use crate::album_qt::AlbumCardData;

#[derive(Clone, Serialize)]
pub struct SidepanelSection {
    pub title: String,
    pub albums: Vec<AlbumCardData>,
}

#[derive(Clone, Default, Serialize)]
pub struct SidepanelDoc {
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "artistName")]
    pub artist_name: String,
    pub loading: bool,
    pub error: String,
    /// Total release cards across the sections — the reference's
    /// `selected_albums_total`, which is what tells the right pane to draw
    /// "No albums found." instead of an empty scroller.
    pub total: usize,
    pub sections: Vec<SidepanelSection>,
}

/// Monotonic id for the selection currently on screen. A load carries the
/// generation it was started for and is dropped when the user has clicked on —
/// the same guard `artist_qt`'s progressive passes use, and the reason a slow
/// reply cannot repaint a later artist.
static GEN: AtomicU64 = AtomicU64::new(0);

fn publish(doc: SidepanelDoc) {
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".into());
    crate::library_bridge::ui(move |mut b| {
        b.as_mut().set_sidepanel_json(QString::from(json.as_str()));
    });
}

/// Drop the selection (leaving the sidepanel, logout, user switch).
pub fn clear() {
    GEN.fetch_add(1, Ordering::SeqCst);
    publish(SidepanelDoc::default());
}

/// Select an artist in the left rail: paint the loading state immediately,
/// then fetch the release sections off the UI thread.
pub fn select(artist_id: String, artist_name: String) {
    let generation = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    publish(SidepanelDoc {
        artist_id: artist_id.clone(),
        artist_name: artist_name.clone(),
        loading: true,
        ..Default::default()
    });
    let runtime = crate::app();
    crate::spawn(async move {
        let result = crate::artist_qt::load_artist(&runtime, &artist_id).await;
        // The user clicked another artist while this was in flight.
        if GEN.load(Ordering::SeqCst) != generation {
            log::debug!("[qbz-qt] sidepanel artist {artist_id}: superseded, dropping");
            return;
        }
        match result {
            Ok(data) => {
                let sections: Vec<SidepanelSection> = data
                    .release_sections
                    .into_iter()
                    .filter(|s| !s.cards.is_empty())
                    .map(|s| SidepanelSection {
                        title: s.title,
                        albums: s.cards,
                    })
                    .collect();
                let total: usize = sections.iter().map(|s| s.albums.len()).sum();
                log::info!(
                    "[qbz-qt] sidepanel artist {artist_id}: {total} release(s) in {} section(s)",
                    sections.len()
                );
                publish(SidepanelDoc {
                    artist_id,
                    artist_name,
                    loading: false,
                    error: String::new(),
                    total,
                    sections,
                });
            }
            Err(e) => {
                log::error!("[qbz-qt] sidepanel artist {artist_id} load failed: {e}");
                publish(SidepanelDoc {
                    artist_id,
                    artist_name,
                    loading: false,
                    error: e,
                    total: 0,
                    sections: Vec::new(),
                });
            }
        }
    });
}
