//! Late materialization of a Qobuz queue row from a purchased album copy.
//!
//! Contract `qt-frontend/2026-09-01-purchases-local-playback/00-CONTRACT.md`
//! §6: the queue keeps its catalog identity (`play_id` is always the Qobuz
//! track id) and only the bytes change. The per-profile album preference is
//! authoritative: Qobuz mode never enters this path; purchase mode considers
//! only complete, healthy copies of the exact selected format.
//!
//! Completeness is checked against the persisted manifest of one `copy_id` at
//! a time. The library accessor performs bounded health probes off the UI
//! thread and returns every viable physical copy in recency order. A copy
//! already used for this album/preference is kept first across the audible and
//! gapless decisions; if it becomes unhealthy, another exact-format complete
//! copy may take over before the ordinary Qobuz tier walk.
//!
//! DSD is DSD: a `.dsf`/`.dff` copy rides the additive `DsdFile` ticket the
//! Local Library already uses. Its start offset is applied through the
//! player's existing direct-DSD seek after the ticket starts; nothing here
//! changes the protected audio path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use qbz_library::PurchasePlaybackMode;
use qbz_source::PlaybackTicket;

/// A registered download of a Qobuz track whose file answered the probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PurchasedCopy {
    pub copy_id: String,
    pub path: PathBuf,
    pub format_id: u32,
}

impl PurchasedCopy {
    /// `.dsf` / `.dff` — the containers the player streams through its DSD
    /// path. Decided by extension exactly as the local source decides it.
    pub fn is_dsd(&self) -> bool {
        is_dsd_path(&self.path)
    }

    /// The ticket that plays this copy under the CATALOG id.
    pub fn ticket(&self, play_id: u64, start_secs: u64) -> PlaybackTicket {
        if self.is_dsd() {
            PlaybackTicket::DsdFile {
                path: self.path.clone(),
                play_id,
            }
        } else {
            PlaybackTicket::File {
                path: self.path.clone(),
                play_id,
                seek_secs: (start_secs > 0).then_some(start_secs as f64),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PurchaseResolution {
    pub album_id: String,
    pub format_id: u32,
    preference_updated_at: i64,
    pub copies: Vec<PurchasedCopy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PinnedCopy {
    format_id: u32,
    preference_updated_at: i64,
    copy_id: String,
}

fn pinned_copies() -> &'static Mutex<HashMap<String, PinnedCopy>> {
    static PINNED: OnceLock<Mutex<HashMap<String, PinnedCopy>>> = OnceLock::new();
    PINNED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Exact purchased format that supplied the bytes currently associated with
/// a catalog track id. The queue deliberately keeps Qobuz metadata, so this
/// small side table is the only honest source for the NPB quality stamp after
/// late materialization (including a successor prepared gaplessly).
fn materialized_formats() -> &'static Mutex<HashMap<u64, u32>> {
    static FORMATS: OnceLock<Mutex<HashMap<u64, u32>>> = OnceLock::new();
    FORMATS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn materialized_format(track_id: u64) -> Option<u32> {
    materialized_formats()
        .lock()
        .ok()
        .and_then(|formats| formats.get(&track_id).copied())
}

pub(crate) fn clear_materialized_track(track_id: u64) {
    if let Ok(mut formats) = materialized_formats().lock() {
        formats.remove(&track_id);
    }
}

fn is_dsd_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("dsf") || e.eq_ignore_ascii_case("dff"))
        .unwrap_or(false)
}

fn prefer_pinned_copy(copies: &mut [PurchasedCopy], pinned_copy_id: Option<&str>) {
    let Some(copy_id) = pinned_copy_id else {
        return;
    };
    if let Some(index) = copies.iter().position(|copy| copy.copy_id == copy_id) {
        copies.swap(0, index);
    }
}

/// Resolve all viable copies for this exact logical album/track preference.
/// Returning `None` in Qobuz mode is intentional and prevents purchase probes
/// from delaying the normal cache/offline/network tier walk.
pub(crate) async fn resolve_preferred_copies(
    track_id: u64,
    album_id: Option<&str>,
) -> Option<PurchaseResolution> {
    let album_id = album_id?.trim().to_string();
    if album_id.is_empty() {
        return None;
    }
    let db_track_id = i64::try_from(track_id).ok()?;
    let query_album_id = album_id.clone();
    let (format_id, preference_updated_at, rows) = tokio::task::spawn_blocking(move || {
        crate::library_db_qt::with_db(false, |db| {
            let preference = db.purchase_playback_preference(&query_album_id)?;
            if preference.mode != PurchasePlaybackMode::Purchase {
                return Ok(None);
            }
            let Some(format_id) = preference.format_id.filter(|format_id| *format_id > 0) else {
                return Ok(None);
            };
            let rows = db.complete_healthy_purchase_track_candidates(
                &query_album_id,
                format_id,
                db_track_id,
            )?;
            Ok(Some((format_id as u32, preference.updated_at, rows)))
        })
        .flatten()
    })
    .await
    .ok()
    .flatten()?;

    let mut copies: Vec<PurchasedCopy> = rows
        .into_iter()
        .map(|(copy, track)| PurchasedCopy {
            copy_id: copy.copy_id,
            path: PathBuf::from(track.file_path),
            format_id,
        })
        .collect();
    if copies.is_empty() {
        log::info!(
            "[qbz-qt] purchase: no complete healthy copy for album {album_id}, track {track_id}, format {format_id}; using Qobuz"
        );
        return None;
    }

    let pinned = pinned_copies().lock().ok().and_then(|pins| {
        pins.get(&album_id)
            .filter(|pin| {
                pin.format_id == format_id && pin.preference_updated_at == preference_updated_at
            })
            .map(|pin| pin.copy_id.clone())
    });
    prefer_pinned_copy(&mut copies, pinned.as_deref());
    Some(PurchaseResolution {
        album_id,
        format_id,
        preference_updated_at,
        copies,
    })
}

/// Keep current and gapless materialization on one physical copy whenever it
/// remains healthy. Called only after the player accepted the ticket.
pub(crate) fn remember_materialized_copy(
    track_id: u64,
    resolution: &PurchaseResolution,
    copy: &PurchasedCopy,
) {
    if let Ok(mut pins) = pinned_copies().lock() {
        pins.insert(
            resolution.album_id.clone(),
            PinnedCopy {
                format_id: resolution.format_id,
                preference_updated_at: resolution.preference_updated_at,
                copy_id: copy.copy_id.clone(),
            },
        );
    }
    if let Ok(mut formats) = materialized_formats().lock() {
        formats.insert(track_id, copy.format_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dsd_copy_rides_the_dsd_ticket_under_the_catalog_id() {
        let copy = PurchasedCopy {
            copy_id: "copy-a".to_string(),
            path: PathBuf::from("/m/Various Artists/A [DSF][DSD128]/01 - x.dsf"),
            format_id: 56,
        };
        assert!(copy.is_dsd());
        match copy.ticket(189_898_763, 0) {
            PlaybackTicket::DsdFile { play_id, path } => {
                assert_eq!(play_id, 189_898_763, "the Qobuz id, never a local row id");
                assert_eq!(path, copy.path);
            }
            other => panic!(
                "expected a DsdFile ticket, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn a_flac_copy_rides_the_file_ticket_and_carries_the_start_offset() {
        let copy = PurchasedCopy {
            copy_id: "copy-a".to_string(),
            path: PathBuf::from("/m/A/B [FLAC][24-bit,96kHz]/01 - x.FLAC"),
            format_id: 7,
        };
        assert!(!copy.is_dsd());
        match copy.ticket(42, 0) {
            PlaybackTicket::File { seek_secs, .. } => assert_eq!(seek_secs, None),
            _ => panic!("expected a File ticket"),
        }
        match copy.ticket(42, 30) {
            PlaybackTicket::File {
                seek_secs, play_id, ..
            } => {
                assert_eq!(seek_secs, Some(30.0));
                assert_eq!(play_id, 42);
            }
            _ => panic!("expected a File ticket"),
        }
    }

    #[test]
    fn the_pinned_copy_is_tried_before_newer_siblings() {
        let mut copies = vec![
            PurchasedCopy {
                copy_id: "new".to_string(),
                path: PathBuf::from("/new/01.dsf"),
                format_id: 55,
            },
            PurchasedCopy {
                copy_id: "playing".to_string(),
                path: PathBuf::from("/playing/01.dsf"),
                format_id: 55,
            },
        ];
        prefer_pinned_copy(&mut copies, Some("playing"));
        let order: Vec<String> = copies.into_iter().map(|copy| copy.copy_id).collect();
        assert_eq!(order, vec!["playing", "new"]);
    }
}
