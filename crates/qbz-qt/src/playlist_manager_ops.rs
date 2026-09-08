//! Playlist Manager, controller part 3 — the optimistic mutations: favourite,
//! hidden, move-to-folder and the arrow reorder, plus their persistence and
//! their sidebar hop.
//!
//! Port of the `PlaylistManagerActions` per-card handlers in the reference's
//! `main.rs:5335-5535` (`on_toggle_favorite` / `on_toggle_hidden` /
//! `on_move_to_folder` / `on_move_up` / `on_move_down`) plus
//! `playlist_manager.rs`'s `*_local` cache patches and `reorder_step`.
//!
//! # The shape every mutation here follows (§5.3) — and why
//!
//! ```text
//! 1. patch the manager cache      — ONLY when it is warm
//! 2. publish the manager document — ONLY when step 1 happened
//! 3. write the DB                 — ALWAYS
//! 4. hop the sidebar              — ALWAYS (per the D10 table)
//! ```
//!
//! Steps 3 and 4 must never sit behind a `CACHE.is_some()` test, and there
//! must never be an early return at the top of one of these functions. Blocks
//! 2, 3 and 5 mount these same verbs behind SIDEBAR surfaces, where the
//! manager view has typically never been opened and the cache is therefore
//! `None`. Mirroring `reload_if_loaded()`'s early return here turns "Hide from
//! sidebar" and "Move to folder" into silent no-ops for exactly the
//! account-less, offline population local playlists exist for.
//!
//! The corollary is that the new value cannot come from the cache alone: when
//! the cache is cold each toggle re-reads the CURRENT flag inside the same
//! `spawn_blocking` that writes it, and negates that. A toggle that cannot see
//! the old value is not a toggle.
//!
//! # Which sidebar verb, per mutation (D10)
//!
//! | mutation | sidebar |
//! |---|---|
//! | favourite | **none** — not a sidebar-visible property |
//! | hidden (either kind) | `crate::reload_sidebar_including_local()` |
//! | move to folder (either kind) | `sidebar_qt::move_playlist_optimistic` + `crate::publish_sidebar()` |
//! | reorder | **none** — the sidebar picks the positions up on its next load |
//!
//! `crate::reload_sidebar()` — the "refetch from Qobuz" verb — is NOT used by
//! any of them: its first statement is an offline bail, so it is a no-op for
//! precisely the users these writes are for.

use crate::folders_qt;
use crate::local_playlist_qt;
use crate::local_playlist_qt::PlaylistRef;
use crate::playlist_manager_qt::{patch_cache, publish_document, visible_qobuz_ids};

// ---------------------------------------------------------------------------
// Favourite
// ---------------------------------------------------------------------------

/// Flip the cached favourite flag. `Some(new_value)` when the cache was warm
/// AND held the row; `None` otherwise, which is not an error — it means the
/// DB write below has to derive the value itself.
fn patch_favorite(id: &str) -> Option<bool> {
    let mut out = None;
    patch_cache(|data| match PlaylistRef::parse(id) {
        Some(PlaylistRef::Local(local)) => {
            if let Some(p) = data.locals.iter_mut().find(|p| p.id == local) {
                p.is_favorite = !p.is_favorite;
                out = Some(p.is_favorite);
            }
        }
        Some(PlaylistRef::Qobuz(pid)) => {
            if let Some(p) = data.playlists.iter_mut().find(|p| p.id == pid) {
                p.is_favorite = !p.is_favorite;
                out = Some(p.is_favorite);
            }
        }
        None => {}
    });
    out
}

/// Toggle a playlist's favourite flag.
///
/// A LOCAL playlist keeps its flag on its own `local_playlists` row (B3): the
/// `playlist_settings` table's primary key is a `u64` Qobuz id, which a
/// `local:<uuid>` can never be. No sidebar refresh — the heart is not a
/// sidebar-visible property.
pub(crate) fn toggle_favorite(id: &str) {
    let optimistic = patch_favorite(id);
    if optimistic.is_some() {
        publish_document();
    }
    let id = id.to_string();
    crate::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || match PlaylistRef::parse(&id) {
            Some(PlaylistRef::Local(local)) => {
                let value = optimistic.unwrap_or_else(|| {
                    !local_playlist_qt::get_blocking(&local)
                        .map(|p| p.favorite)
                        .unwrap_or(false)
                });
                local_playlist_qt::set_favorite_blocking(&local, value);
            }
            Some(PlaylistRef::Qobuz(pid)) => {
                let value = optimistic.unwrap_or_else(|| {
                    !folders_qt::playlist_settings_map()
                        .get(&pid)
                        .map(|s| s.is_favorite)
                        .unwrap_or(false)
                });
                folders_qt::set_favorite(pid, value);
            }
            None => {}
        })
        .await;
    });
}

// ---------------------------------------------------------------------------
// Hidden
// ---------------------------------------------------------------------------

fn patch_hidden(id: &str) -> Option<bool> {
    let mut out = None;
    patch_cache(|data| match PlaylistRef::parse(id) {
        Some(PlaylistRef::Local(local)) => {
            if let Some(p) = data.locals.iter_mut().find(|p| p.id == local) {
                p.is_hidden = !p.is_hidden;
                out = Some(p.is_hidden);
            }
        }
        Some(PlaylistRef::Qobuz(pid)) => {
            if let Some(p) = data.playlists.iter_mut().find(|p| p.id == pid) {
                p.is_hidden = !p.is_hidden;
                out = Some(p.is_hidden);
            }
        }
        None => {}
    });
    out
}

/// Toggle a playlist's hidden flag, then refresh the sidebar — hidden
/// playlists drop out of the tree, so it must re-render.
///
/// The refresh verb is `reload_sidebar_including_local()`, the one that has no
/// offline gate. Hiding is undoable ONLY from the manager's All/Visible/Hidden
/// filter, so this landing without that filter would make hiding irreversible
/// from inside QBZ — which is why block 5 (the sidebar row menu's Hide row)
/// depends on block 4.
pub(crate) fn toggle_hidden(id: &str) {
    let optimistic = patch_hidden(id);
    if optimistic.is_some() {
        publish_document();
    }
    let id = id.to_string();
    crate::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || match PlaylistRef::parse(&id) {
            Some(PlaylistRef::Local(local)) => {
                let value = optimistic.unwrap_or_else(|| {
                    !local_playlist_qt::get_blocking(&local)
                        .map(|p| p.hidden)
                        .unwrap_or(false)
                });
                local_playlist_qt::set_hidden_blocking(&local, value);
            }
            Some(PlaylistRef::Qobuz(pid)) => {
                let value = optimistic.unwrap_or_else(|| {
                    !folders_qt::playlist_settings_map()
                        .get(&pid)
                        .map(|s| s.hidden)
                        .unwrap_or(false)
                });
                folders_qt::set_hidden(pid, value);
            }
            None => {}
        })
        .await;
        // AFTER the write settles, inside the same task — otherwise the
        // rebuild reads the pre-write state (reference: main.rs:5386-5394).
        crate::reload_sidebar_including_local();
    });
}

// ---------------------------------------------------------------------------
// Move to folder
// ---------------------------------------------------------------------------

/// Patch the cached folder membership. `""` = root. Returns whether the cache
/// was warm and held the row.
fn patch_folder(id: &str, folder_id: &str) -> bool {
    let mut patched = false;
    let target = (!folder_id.is_empty()).then(|| folder_id.to_string());
    patch_cache(|data| match PlaylistRef::parse(id) {
        Some(PlaylistRef::Local(local)) => {
            if let Some(p) = data.locals.iter_mut().find(|p| p.id == local) {
                p.folder_id = target;
                patched = true;
            }
        }
        Some(PlaylistRef::Qobuz(pid)) => {
            if let Some(p) = data.playlists.iter_mut().find(|p| p.id == pid) {
                p.folder_id = target;
                patched = true;
            }
        }
        None => {}
    });
    patched
}

/// Move a playlist into `folder_id`, or to root when it is `""`.
///
/// Both id kinds are handled (D6.4): a `local:<uuid>` routes to
/// `local_playlists.folder_id`, which points at the SAME `playlist_folders`
/// table the Qobuz playlists use, so one folder holds both kinds.
pub(crate) fn move_to_folder(playlist_id: &str, folder_id: &str) {
    if patch_folder(playlist_id, folder_id) {
        publish_document();
    }
    let id = playlist_id.to_string();
    let fid = folder_id.to_string();
    crate::spawn(async move {
        let write_id = id.clone();
        let write_fid = fid.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let target = (!write_fid.is_empty()).then_some(write_fid.as_str());
            match PlaylistRef::parse(&write_id) {
                Some(PlaylistRef::Local(local)) => {
                    local_playlist_qt::move_to_folder_blocking(&local, target);
                }
                Some(PlaylistRef::Qobuz(pid)) => folders_qt::move_playlist(pid, target),
                None => {}
            }
        })
        .await;
        // Patch + republish rather than reload: the offline-safe reload verb
        // still re-reads the whole DB for a change we already know, and the
        // network one is a no-op offline (D10).
        crate::sidebar_qt::move_playlist_optimistic(&id, &fid);
        crate::publish_sidebar();
    });
}

// ---------------------------------------------------------------------------
// Arrow reorder (custom sort)
// ---------------------------------------------------------------------------

/// Move a playlist one slot up in the custom order.
pub(crate) fn move_up(id: &str) {
    reorder_step(id, -1);
}

/// Move a playlist one slot down in the custom order.
pub(crate) fn move_down(id: &str) {
    reorder_step(id, 1);
}

/// Swap a playlist with its neighbour in the CURRENTLY VISIBLE order and
/// renumber `position = 0..n` over that vector.
///
/// Two things here look like bugs and are faithful ports (D9):
///
/// * Positions are renumbered over the **visible subset only**, so rows the
///   filter removed keep their old positions and can collide. A collision
///   degrades to stable order, not corruption. Do not "fix" it.
/// * A LOCAL playlist is not reorderable at all — it sorts at `i64::MAX` and
///   `playlist_settings` has no row it could occupy — so its `parse::<u64>()`
///   fails here and the chevrons are not rendered on its card (D29).
///
/// A no-op (id not visible, or already at the end) writes NOTHING — the same
/// rule as the reference's "empty order ⇒ skip the DB write". With a cold
/// cache the visible order is empty, so every call is that no-op; the arrows
/// are only ever drawn by the manager view, which cannot render without the
/// document that fills the cache.
fn reorder_step(id: &str, delta: i32) {
    let Ok(pid) = id.parse::<u64>() else {
        return;
    };
    let mut ids = visible_qobuz_ids();
    let Some(pos) = ids.iter().position(|&x| x == pid) else {
        return;
    };
    let target = pos as i32 + delta;
    if target < 0 || target as usize >= ids.len() {
        return;
    }
    ids.swap(pos, target as usize);

    if patch_cache(|data| {
        for (i, moved) in ids.iter().enumerate() {
            if let Some(p) = data.playlists.iter_mut().find(|p| p.id == *moved) {
                p.position = i as i32;
            }
        }
    }) {
        publish_document();
    }

    crate::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || folders_qt::reorder_playlists(&ids)).await;
    });
}
