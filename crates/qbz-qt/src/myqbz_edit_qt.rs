//! My QBZ — collection detail EDIT operations. The Qt port of
//! `crates/qbz/src/myqbz_edit.rs` (spec 02 §6.4), publishing the small
//! `QbzMyQbz.editJson` modal document (spec 02 §5.5).
//!
//! Wires the hero overflow (⋯) menu + the Rename / Description /
//! Delete-confirm panels to the shared `qbz_mixtape::repo` setters, reached
//! through `library_db_qt::with_db(true, …)` — the only CREATE-capable
//! accessor, which a fresh account needs (spec 02 §2.1).
//!
//! Every mutation then RELOADS the open detail (the reference's "-> reload") so
//! the hero + toolbar reflect the change, and re-lists BOTH grids
//! (`myqbz_qt::reload_grids`): `nav_qt::back()` in this port publishes only
//! `currentView` and never re-dispatches a load, so a user pressing BACK after a
//! rename / convert / bulk-remove would otherwise land on a stale grid card
//! (spec 02 §7 T4).
//!
//! All DB work runs inside `spawn_blocking` (T1); the toast + reload hop back
//! through their own publishers.

use serde::Serialize;

use cxx_qt_lib::QString;
use qbz_models::mixtape::{CollectionKind, CollectionPlayMode};

// ---------------------------------------------------------------------------
// Document (spec 02 §5.5)
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize)]
pub struct EditDoc {
    /// "" | "rename" | "description" | "delete".
    pub mode: String,
    pub open: bool,
    /// Rename prefill AND the delete-confirm interpolation.
    pub name: String,
    /// Description prefill.
    pub description: String,
    pub busy: bool,
    /// Aliases — spec 01 §13.1 spells the two prefills
    /// `draftName` / `draftDescription`. Filled in `publish`, so either QML
    /// spelling resolves.
    #[serde(rename = "draftName")]
    pub draft_name: String,
    #[serde(rename = "draftDescription")]
    pub draft_description: String,
}

static DOC: std::sync::Mutex<Option<EditDoc>> = std::sync::Mutex::new(None);

fn with_doc<R>(f: impl FnOnce(&mut EditDoc) -> R) -> R {
    let mut guard = DOC.lock().unwrap();
    let doc = guard.get_or_insert_with(EditDoc::default);
    f(doc)
}

fn publish(doc: &EditDoc) {
    let mut out = doc.clone();
    out.draft_name = out.name.clone();
    out.draft_description = out.description.clone();
    let json = serde_json::to_string(&out).unwrap_or_else(|_| "{}".into());
    crate::myqbz_bridge::ui(move |mut b| {
        b.as_mut().set_edit_json(QString::from(json.as_str()));
    });
}

fn set_busy(busy: bool) {
    let doc = with_doc(|d| {
        d.busy = busy;
        d.clone()
    });
    publish(&doc);
}

/// Close the modal (clears mode + busy).
pub(crate) fn close() {
    let doc = with_doc(|d| {
        d.open = false;
        d.mode = String::new();
        d.busy = false;
        d.clone()
    });
    publish(&doc);
}

// NO `republish()` here, deliberately. `main.rs::apply_language` republishes
// `myqbz_qt` / `myqbz_detail_qt` / `myqbz_builder_qt` because those documents
// carry Rust-BUILT translated strings (the kind eyebrow, the "{} albums" plural,
// unresolved rows' TYPE label, the builder's footer). `EditDoc` carries none —
// `mode`, `name`, `description` and `busy` are all data, and every visible label
// in the panels is a QML `QbzSession.tr(msgid, trRev)` binding that re-evaluates
// itself. The reference agrees: its language switch (`reseed_i18n_labels`,
// `qbz/src/main.rs:4982`) reseeds only the AppearanceState string arrays and
// never touches `MyQbzEditState`.

/// Drop the modal state on logout.
pub(crate) fn teardown() {
    let doc = with_doc(|d| {
        *d = EditDoc::default();
        d.clone()
    });
    publish(&doc);
}

// ---------------------------------------------------------------------------
// Panel openers — the prefills come from the RUST document, never from QML
// ---------------------------------------------------------------------------

fn open_panel(mode: &str) {
    let name = crate::myqbz_detail_qt::current_name();
    let description = crate::myqbz_detail_qt::current_description();
    let doc = with_doc(|d| {
        d.mode = mode.to_string();
        d.open = true;
        d.busy = false;
        d.name = name.clone();
        d.description = description.clone();
        d.clone()
    });
    publish(&doc);
}

pub(crate) fn open_rename() {
    open_panel("rename");
}

pub(crate) fn open_description() {
    open_panel("description");
}

pub(crate) fn open_delete() {
    open_panel("delete");
}

// ---------------------------------------------------------------------------
// DB write helper
// ---------------------------------------------------------------------------

/// Run one repo mutation against the per-user library.db. `Err` on any failure
/// (DB unavailable or the repo error). SYNCHRONOUS — always called inside
/// `spawn_blocking`.
fn write_repo<F>(f: F) -> Result<(), String>
where
    F: FnOnce(&qbz_library::LibraryDatabase) -> Result<(), String>,
{
    crate::library_db_qt::with_db(true, |db| Ok(f(db)))
        .unwrap_or_else(|| Err("library database unavailable".to_string()))
}

// ---------------------------------------------------------------------------
// Reload after a landed mutation
// ---------------------------------------------------------------------------

/// The reference's "-> reload": re-run the detail load for the same id WITHOUT
/// a second nav record, and re-list both grids (T4).
fn reload(id: String) {
    crate::myqbz_detail_qt::reload(id);
    crate::myqbz_qt::reload_grids();
}

/// On success: optional success toast, close the modal, reload. On failure:
/// clear busy + error toast (the modal stays open).
fn finish(id: String, result: Result<(), String>, success_toast: Option<String>, err_msg: String) {
    match result {
        Ok(()) => {
            if let Some(msg) = success_toast {
                crate::toast_qt::success(msg);
            }
            close();
            reload(id);
        }
        Err(e) => {
            log::warn!("[qbz-qt] myqbz_edit mutation failed: {e}");
            set_busy(false);
            crate::toast_qt::error(err_msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Rename submit: trim + cap at 80 Unicode scalars; trimmed-empty closes
/// WITHOUT writing.
pub(crate) fn submit_rename(raw_name: String) {
    let id = crate::myqbz_detail_qt::current_id();
    let name: String = raw_name.trim().chars().take(80).collect();
    if id.is_empty() || name.is_empty() {
        close();
        return;
    }
    set_busy(true);
    crate::spawn(async move {
        let write_id = id.clone();
        let write_name = name.clone();
        let result = tokio::task::spawn_blocking(move || {
            write_repo(|db| {
                db.with_connection(|conn| {
                    qbz_mixtape::repo::rename_collection(conn, &write_id, &write_name)
                })
                .map_err(|e| e.to_string())
            })
        })
        .await
        .unwrap_or_else(|e| Err(format!("rename task panicked: {e}")));

        finish(id, result, None, qbz_i18n::t("Failed to rename"));
    });
}

/// Description submit: trimmed-empty ⇒ SQL NULL (clear).
pub(crate) fn submit_description(raw_description: String) {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() {
        close();
        return;
    }
    let trimmed = raw_description.trim().to_string();
    let desc: Option<String> = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    set_busy(true);
    crate::spawn(async move {
        let write_id = id.clone();
        let write_desc = desc.clone();
        let result = tokio::task::spawn_blocking(move || {
            write_repo(|db| {
                db.with_connection(|conn| {
                    qbz_mixtape::repo::set_description(conn, &write_id, write_desc.as_deref())
                })
                .map_err(|e| e.to_string())
            })
        })
        .await
        .unwrap_or_else(|e| Err(format!("description task panicked: {e}")));

        finish(id, result, None, qbz_i18n::t("Failed to save description"));
    });
}

/// Hero overflow play-mode toggle: `in_order` ⇒ `AlbumShuffle`, anything else ⇒
/// `InOrder`. The CURRENT mode is read from the Rust document, not from QML.
pub(crate) fn toggle_play_mode() {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() {
        return;
    }
    let next = if crate::myqbz_detail_qt::current_play_mode() == "in_order" {
        CollectionPlayMode::AlbumShuffle
    } else {
        CollectionPlayMode::InOrder
    };
    crate::spawn(async move {
        let write_id = id.clone();
        let result = tokio::task::spawn_blocking(move || {
            write_repo(|db| {
                db.with_connection(|conn| qbz_mixtape::repo::set_play_mode(conn, &write_id, next))
                    .map_err(|e| e.to_string())
            })
        })
        .await
        .unwrap_or_else(|e| Err(format!("play-mode task panicked: {e}")));

        finish(id, result, None, qbz_i18n::t("Failed to change play mode"));
    });
}

/// Hero overflow convert-kind: `mixtape` ⇄ `collection`. Any other current kind
/// (i.e. `artist_collection`) toasts immediately and writes NOTHING — the repo
/// rejects it either way.
pub(crate) fn convert_kind() {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() {
        return;
    }
    let next = match crate::myqbz_detail_qt::current_kind().as_str() {
        "mixtape" => CollectionKind::Collection,
        "collection" => CollectionKind::Mixtape,
        _ => {
            crate::toast_qt::error(qbz_i18n::t("Cannot convert this kind"));
            return;
        }
    };
    crate::spawn(async move {
        let write_id = id.clone();
        let result = tokio::task::spawn_blocking(move || {
            write_repo(|db| {
                db.with_connection(|conn| qbz_mixtape::repo::set_kind(conn, &write_id, next))
                    .map_err(|e| e.to_string())
            })
        })
        .await
        .unwrap_or_else(|e| Err(format!("convert-kind task panicked: {e}")));

        match result {
            Ok(()) => {
                crate::toast_qt::success(qbz_i18n::t("Converted"));
                // convert_kind MOVES the row between the two grids, so both
                // must republish (T4).
                reload(id);
            }
            Err(e) => {
                log::warn!("[qbz-qt] myqbz_edit convert-kind failed: {e}");
                // The repo's only rejection here is the artist_collection guard.
                crate::toast_qt::error(qbz_i18n::t("Cannot convert this kind"));
            }
        }
    });
}

/// Delete-confirm: `repo::delete_collection` (CASCADE). On success the order is
/// exactly: close the modal → drop the stored view-prefs key → `nav_qt::back()`
/// → re-list the grids. On error the modal STAYS open with `busy = false`.
pub(crate) fn confirm_delete() {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() {
        close();
        return;
    }
    set_busy(true);
    crate::spawn(async move {
        let write_id = id.clone();
        let result = tokio::task::spawn_blocking(move || {
            write_repo(|db| {
                db.with_connection(|conn| qbz_mixtape::repo::delete_collection(conn, &write_id))
                    .map_err(|e| e.to_string())
            })
        })
        .await
        .unwrap_or_else(|e| Err(format!("delete task panicked: {e}")));

        match result {
            Ok(()) => {
                close();
                crate::myqbz_prefs_qt::remove_view_prefs(&id);
                // Same lifetime as the view prefs: the collection is gone, so
                // its accordion state goes with it. (Ids are v4 UUIDs, so a
                // re-created collection could not inherit it either way — this
                // keeps the sidecar from accumulating dead entries.)
                crate::myqbz_prefs_qt::remove_open_rows(&id);
                crate::nav_qt::back();
                crate::myqbz_qt::reload_grids();
            }
            Err(e) => {
                log::warn!("[qbz-qt] myqbz_edit delete failed: {e}");
                set_busy(false);
                crate::toast_qt::error(qbz_i18n::t("Failed to delete"));
            }
        }
    });
}

/// Bulk-remove the current select-mode selection.
pub(crate) fn remove_selected() {
    remove_positions(crate::myqbz_detail_qt::selected_positions());
}

/// Per-row "Remove from collection" — routes through the SAME bulk remover with
/// a 1-element vec (spec 02 §4.1), so the compaction rule, the toast and the
/// reload are shared and cannot drift.
pub(crate) fn remove_item(position: i32) {
    remove_positions(vec![position]);
}

/// Remove `positions` from the open collection. They are removed
/// HIGHEST-FIRST so each `repo::remove_item`'s position compaction never shifts
/// a position still to be deleted (T11). Per-item errors are logged and the
/// batch continues; only a hard DB-unavailable surfaces.
fn remove_positions(mut positions: Vec<i32>) {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() || positions.is_empty() {
        return;
    }
    positions.sort_unstable_by(|a, b| b.cmp(a));
    let count = positions.len();
    crate::spawn(async move {
        let write_id = id.clone();
        let result = tokio::task::spawn_blocking(move || {
            write_repo(|db| {
                db.with_connection(|conn| {
                    for pos in &positions {
                        if let Err(e) = qbz_mixtape::repo::remove_item(conn, &write_id, *pos) {
                            log::warn!(
                                "[qbz-qt] myqbz_edit remove_item({write_id}, {pos}) failed: {e}"
                            );
                        }
                    }
                });
                Ok(())
            })
        })
        .await
        .unwrap_or_else(|e| Err(format!("bulk-remove task panicked: {e}")));

        match result {
            Ok(()) => {
                crate::myqbz_detail_qt::clear_selection();
                crate::toast_qt::info(qbz_i18n::tf(
                    "Removed {} item",
                    "Removed {} items",
                    count as i64,
                    &[&count.to_string()],
                ));
                reload(id);
            }
            Err(e) => {
                log::warn!("[qbz-qt] myqbz_edit bulk-remove failed: {e}");
                crate::toast_qt::error(qbz_i18n::t("Failed to remove items"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    /// The rename cap is 80 Unicode SCALARS, not bytes — a CJK or accented
    /// name must not be cut mid-character.
    #[test]
    fn rename_caps_at_eighty_scalars() {
        let raw = "  ".to_string() + &"あ".repeat(200) + "  ";
        let capped: String = raw.trim().chars().take(80).collect();
        assert_eq!(capped.chars().count(), 80);
    }

    /// Descending order is what makes the repo's position compaction harmless.
    #[test]
    fn bulk_remove_positions_go_highest_first() {
        let mut positions = vec![0, 5, 2, 9];
        positions.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(positions, vec![9, 5, 2, 0]);
    }
}
