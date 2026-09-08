//! Folder modals controller — the Rust half of `QbzFolderEdit` (contract
//! block 2). Port of `crates/qbz/src/main.rs`'s `CreateFolderActions` /
//! `FolderEditActions` handlers (`:5143-5183`, `:5600-5740`) plus
//! `primitives/CreateFolderModal.slint` and `primitives/FolderEditModal.slint`.
//!
//! # What is state here and what is state in QML
//!
//! Text, preset, colour and the hidden switch are QML-LOCAL drafts; every
//! submit invokable takes them as arguments (`MyQbzModals.qml`'s convention —
//! a colour click has no business making a round trip). What lives HERE is
//! only what QML cannot own:
//!   * the folder id being edited — held in Rust so a republish can never make
//!     the panel save under a different folder;
//!   * the custom image path — the file dialog is native and lives on this
//!     side;
//!   * `busy` / `creating` — they gate the writes, so the writer owns them;
//!   * the preset and swatch constants, published in the document so the list
//!     has exactly ONE definition (§4.5).
//!
//! # `busy` HOLDS THE PANEL OPEN (D22)
//!
//! The reference closes the editor BEFORE the write (`main.rs:5654`), which
//! makes a failed create invisible and unrecoverable and leaves its own `busy`
//! flag dead — nothing ever sets it. Here: `busy: true`, panel stays up,
//! buttons disarmed; close on success, stay open and toast on failure. `close`
//! is refused while busy. That is a deliberate, recorded divergence and it is
//! what gives `busy` a meaning.
//!
//! # Nothing here may be conditional on the manager cache (§5.3)
//!
//! Both panels are reachable from the SIDEBAR, in sessions where the Playlist
//! Manager view has never been mounted and `playlist_manager_qt::CACHE` is
//! `None`. So the DB write, `refresh_folders()` and the sidebar hop run
//! UNCONDITIONALLY; only `reload_if_loaded()` is allowed to early-return, and
//! it does that itself.
//!
//! # The sidebar hop is the OFFLINE-SAFE one (D10)
//!
//! `crate::reload_sidebar()` opens with an `is_offline()` early return, so
//! after creating or deleting a folder it is a no-op for exactly the
//! account-less, offline population local playlists exist for. Every folder
//! mutation here goes through `crate::reload_sidebar_including_local()`, which
//! runs the same rebuild with no gate.
//!
//! # Thread rules (§5.19)
//!
//! `LibraryDatabase` wraps a `!Send` rusqlite connection and every `folders_qt`
//! call opens the DB, so none of them may run on the Qt thread. An invokable
//! only mutates a `Mutex`, publishes, and `crate::spawn`s the rest.

use std::sync::{LazyLock, Mutex};

use cxx_qt_lib::QString;
use serde::Serialize;

use crate::folders_qt;

// ---------------------------------------------------------------------------
// Constants — published, not hardcoded in QML (§4.5)
// ---------------------------------------------------------------------------

/// The seven icon presets, in the reference's order (`main.rs:5143-5160`).
///
/// Two upstream quirks are INTENTIONAL and are kept: `headphones` renders
/// `audio-lines.svg`, and `folder` is not a case in the id -> glyph chain at
/// all — it falls through to the default arm, which is also the unknown-id
/// arm. Both are upstream intent (§4.5).
const PRESETS: [&str; 7] = [
    "heart",
    "star",
    "music",
    "folder",
    "disc",
    "library",
    "headphones",
];

/// The eleven colour swatches, ONE row of 28 px circles (`main.rs:5161-5183`).
/// `""` is the "use the theme accent" entry — the `.slint` comment claiming
/// "Two rows" is wrong; the grid is 11 wide and 388 px across.
const SWATCHES: [&str; 11] = [
    "", "#ef4444", "#f97316", "#f59e0b", "#10b981", "#06b6d4", "#3b82f6", "#a855f7", "#ec4899",
    "#f43f5e", "#64748b",
];

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The small name-only create panel (the sidebar's "New folder").
#[derive(Clone, Debug, Default)]
struct CreateState {
    open: bool,
    creating: bool,
}

/// The full editor. `id == ""` is CREATE mode (title "New folder", no Delete
/// button) — the manager toolbar's "New folder" opens this surface, the
/// sidebar's opens the small one. That split is the reference's.
#[derive(Clone, Debug)]
struct EditState {
    open: bool,
    id: String,
    /// SEEDs, consumed once by QML on the open transition.
    name: String,
    icon_preset: String,
    icon_color: String,
    is_hidden: bool,
    /// RUST-OWNED, read live by QML: the native picker writes them.
    has_custom_image: bool,
    /// The RAW filesystem path. It is percent-encoded into a `file://` URL
    /// only at publish time (D25) — the DB stores the raw path, matching the
    /// shipped Slint build, so the column stays compatible between the two.
    custom_image_path: String,
    busy: bool,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            open: false,
            id: String::new(),
            name: String::new(),
            icon_preset: "folder".into(),
            icon_color: String::new(),
            is_hidden: false,
            has_custom_image: false,
            custom_image_path: String::new(),
            busy: false,
        }
    }
}

static CREATE: LazyLock<Mutex<CreateState>> = LazyLock::new(|| Mutex::new(CreateState::default()));
static EDIT: LazyLock<Mutex<EditState>> = LazyLock::new(|| Mutex::new(EditState::default()));

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CreateDoc {
    open: bool,
    creating: bool,
}

/// One swatch: `{ "value": "#ef4444", "isAccent": false }`. The ACCENT entry
/// is the one whose value is `""` — QML paints it `theme.accent` and stores
/// `""`, which is what "use the theme accent" means all the way down to the DB
/// (D24).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SwatchDoc {
    value: &'static str,
    is_accent: bool,
}

impl SwatchDoc {
    fn all() -> Vec<SwatchDoc> {
        SWATCHES
            .iter()
            .map(|v| SwatchDoc {
                value: v,
                is_accent: v.is_empty(),
            })
            .collect()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditDoc {
    open: bool,
    id: String,
    name: String,
    icon_preset: String,
    icon_color: String,
    is_hidden: bool,
    has_custom_image: bool,
    /// `file://…` percent-encoded, `""` whenever `has_custom_image` is false.
    /// A bare `/home/...` string would be resolved by QML against the
    /// component's `qrc:` base and fail silently.
    custom_image_path: String,
    busy: bool,
    presets: [&'static str; 7],
    swatches: Vec<SwatchDoc>,
}

fn create_doc() -> String {
    let st = CREATE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    serde_json::to_string(&CreateDoc {
        open: st.open,
        creating: st.creating,
    })
    .unwrap_or_else(|_| "{\"open\":false,\"creating\":false}".to_string())
}

fn edit_doc() -> String {
    let st = EDIT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // `hasCustomImage` is the ONLY question the view asks, and the path is
    // emitted as "" whenever it is false — a stale path is reachable, because
    // `update_folder_full`'s `None` means "leave the image unchanged", so
    // "custom vs preset" must never be derived from the path alone (D25).
    let url = if st.has_custom_image && !st.custom_image_path.is_empty() {
        crate::artwork_qt::file_url(&st.custom_image_path)
    } else {
        String::new()
    };
    serde_json::to_string(&EditDoc {
        open: st.open,
        id: st.id,
        name: st.name,
        icon_preset: st.icon_preset,
        icon_color: st.icon_color,
        is_hidden: st.is_hidden,
        has_custom_image: st.has_custom_image && !st.custom_image_path.is_empty(),
        custom_image_path: url,
        busy: st.busy,
        presets: PRESETS,
        swatches: SwatchDoc::all(),
    })
    .unwrap_or_else(|_| "{\"open\":false}".to_string())
}

fn publish_create() {
    let json = create_doc();
    crate::folder_edit_bridge::ui(move |mut b| {
        b.as_mut().set_create_json(QString::from(json.as_str()));
    });
}

fn publish_edit() {
    let json = edit_doc();
    crate::folder_edit_bridge::ui(move |mut b| {
        b.as_mut().set_edit_json(QString::from(json.as_str()));
    });
}

/// Both documents. Called from `boot()` so the preset + swatch constants are
/// on the QML side before the first open.
pub(crate) fn publish_all() {
    publish_create();
    publish_edit();
}

// ---------------------------------------------------------------------------
// The shared post-write refresh
// ---------------------------------------------------------------------------

/// Everything a folder mutation has to touch, in the order D10 requires: the
/// write has already landed by the time this runs, so nothing reads pre-write
/// state.
///
/// * `refresh_folders()` — cache-INDEPENDENT (D5): a folder created from the
///   sidebar must reach `foldersJson` in a session where the manager view was
///   never opened.
/// * `reload_if_loaded()` — the ONE call allowed to early-return on a cold
///   manager cache; it costs nothing when the manager has no reader.
/// * `reload_sidebar_including_local()` — the OFFLINE-SAFE sidebar verb.
///   `reload_sidebar()` bails while offline, which would make creating or
///   deleting a folder change nothing on screen for the account-less
///   population this whole feature exists for.
fn after_write() {
    crate::playlist_manager_qt::refresh_folders();
    crate::playlist_manager_qt::reload_if_loaded();
    crate::reload_sidebar_including_local();
}

// ---------------------------------------------------------------------------
// Create panel
// ---------------------------------------------------------------------------

/// Sidebar `...` -> "New folder".
pub(crate) fn open_create() {
    {
        let mut st = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        st.open = true;
        st.creating = false;
    }
    publish_create();
}

/// The name is QML-local and arrives here as an argument. Validation stays
/// deliberately weak (§5.25): no duplicate check, no maximum length — the one
/// gate is a non-blank name, and the button reads that same gate so a
/// whitespace-only name shows as disabled instead of lying.
pub(crate) fn create_submit(name: &str) {
    let trimmed = name.trim().to_string();
    {
        let mut st = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        if !st.open || st.creating || trimmed.is_empty() {
            return;
        }
        st.creating = true;
    }
    publish_create();

    crate::spawn(async move {
        // Preset "folder" + colour "" (= the theme accent). Since D24's fix
        // that empty string REACHES the DB, so a sidebar-created folder is
        // accent-coloured instead of the DB default indigo — intended.
        let created = tokio::task::spawn_blocking(move || {
            folders_qt::create_folder_full(&trimmed, "folder", "").is_some()
        })
        .await
        .unwrap_or(false);

        {
            let mut st = CREATE.lock().unwrap_or_else(|e| e.into_inner());
            st.creating = false;
            // Failure keeps the panel OPEN so the name is not lost (D22).
            st.open = !created;
        }
        publish_create();

        if created {
            after_write();
        } else {
            log::warn!("[qbz-qt] folder create failed");
            crate::toast_qt::error(qbz_i18n::t("Failed to create folder"));
        }
    });
}

/// Dismiss. Refused mid-create — the panel is the only thing reporting that a
/// create is in flight.
pub(crate) fn create_close() {
    {
        let mut st = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        if st.creating {
            return;
        }
        st.open = false;
    }
    publish_create();
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

/// Manager toolbar -> "New folder": the FULL editor in create mode.
pub(crate) fn open_new_folder() {
    {
        let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
        *st = EditState {
            open: true,
            ..EditState::default()
        };
    }
    publish_edit();
}

/// Open the editor on an existing folder.
///
/// Reads `folders_qt::load_folders_full()` DIRECTLY. The reference resolves
/// the row out of a manager cache that only a network fetch fills
/// (`playlist_manager.rs`), so even in Slint the sidebar's "Edit folder"
/// silently does nothing until the manager has been opened once in that
/// session. That bug is not ported.
pub(crate) fn open_editor(folder_id: &str) {
    let id = folder_id.to_string();
    if id.is_empty() {
        return;
    }
    crate::spawn(async move {
        let found = tokio::task::spawn_blocking(move || {
            folders_qt::load_folders_full()
                .into_iter()
                .find(|f| f.id == id)
        })
        .await
        .ok()
        .flatten();

        let Some(f) = found else {
            // Deleted underneath us (or the DB is unreadable). Opening an
            // empty editor on a folder that no longer exists would be worse
            // than doing nothing.
            log::warn!("[qbz-qt] folder editor: folder not found");
            return;
        };

        {
            let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
            let path = f.custom_image_path.unwrap_or_default();
            // BOTH conditions, matching `playlist_manager.rs:505`.
            let has_image = f.icon_type == "custom" && !path.is_empty();
            *st = EditState {
                open: true,
                id: f.id,
                name: f.name,
                icon_preset: if f.icon_preset.is_empty() {
                    "folder".into()
                } else {
                    f.icon_preset
                },
                icon_color: f.icon_color,
                is_hidden: f.is_hidden,
                has_custom_image: has_image,
                custom_image_path: if has_image { path } else { String::new() },
                busy: false,
            };
        }
        publish_edit();
    });
}

/// Native picker. Cancel is a silent no-op (reference parity).
///
/// The filter carries `webp` and `gif` as the contract's §5.24 snippet does.
/// This crate builds `image` with `jpeg` + `png` only, so those two can never
/// produce a `QbzSession.artScaled` derivative — `artwork_qt::scaled_path`
/// returns `None` and the host keeps the original source, i.e. the image still
/// displays, at full decode size. Recorded rather than silently narrowed.
pub(crate) fn pick_image() {
    crate::spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title(&qbz_i18n::t("Choose a folder image"))
            .add_filter(
                &qbz_i18n::t("Images"),
                &["png", "jpg", "jpeg", "webp", "gif"],
            )
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_string_lossy().to_string();
        {
            let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
            // The panel may have been dismissed while the native dialog was
            // up; writing into a closed editor would surface on its NEXT open.
            if !st.open {
                return;
            }
            st.custom_image_path = path;
            st.has_custom_image = true;
        }
        publish_edit();
    });
}

/// Drop the custom image. QML calls this alongside its own local preset write
/// — which is what makes the reference's declared-but-never-called
/// `clear-image` callback (`main.rs:5611-5619`) actually live.
pub(crate) fn clear_image() {
    {
        let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
        if !st.open {
            return;
        }
        st.custom_image_path.clear();
        st.has_custom_image = false;
    }
    publish_edit();
}

/// The image tri-state and the two create-mode gaps (§5.22).
///
/// `icon_type` / `custom_image_path` are always ASSERTED, never left unchanged:
/// `None` (= leave alone) is reserved for `set_folder_hidden`, which must not
/// disturb the image.
///
/// `create_folder_full` cannot take an image or the hidden flag, so a custom
/// image picked in create mode is silently lost in the reference and
/// `is_hidden` is dropped. One extra DB write closes both gaps — cheaper than
/// an affordance that offers a picker and discards the result.
fn save_blocking(
    id: &str,
    name: &str,
    preset: &str,
    color: &str,
    image: &str,
    hidden: bool,
) -> bool {
    let icon_type = if image.is_empty() { "preset" } else { "custom" };
    let img = if image.is_empty() {
        Some(None)
    } else {
        Some(Some(image))
    };
    if id.is_empty() {
        match folders_qt::create_folder_full(name, preset, color) {
            Some(f) => {
                if image.is_empty() && !hidden {
                    true
                } else {
                    folders_qt::update_folder_full(
                        &f.id, name, icon_type, preset, color, img, hidden,
                    )
                }
            }
            None => false,
        }
    } else {
        folders_qt::update_folder_full(id, name, icon_type, preset, color, img, hidden)
    }
}

pub(crate) fn edit_save(name: &str, icon_preset: &str, icon_color: &str, is_hidden: bool) {
    let trimmed = name.trim().to_string();
    let (id, image) = {
        let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
        if !st.open || st.busy || trimmed.is_empty() {
            return;
        }
        st.busy = true;
        (st.id.clone(), st.custom_image_path.clone())
    };
    publish_edit();

    let is_create = id.is_empty();
    let preset = icon_preset.to_string();
    let color = icon_color.to_string();
    crate::spawn(async move {
        let ok = tokio::task::spawn_blocking(move || {
            save_blocking(&id, &trimmed, &preset, &color, &image, is_hidden)
        })
        .await
        .unwrap_or(false);

        {
            let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
            st.busy = false;
            // Stay OPEN on failure, with everything the user typed intact.
            st.open = !ok;
        }
        publish_edit();

        if ok {
            after_write();
        } else {
            log::warn!("[qbz-qt] folder save failed (create={is_create})");
            crate::toast_qt::error(qbz_i18n::t(if is_create {
                "Failed to create folder"
            } else {
                "Failed to save folder"
            }));
        }
    });
}

/// The confirm sub-modal is QML-local (`QbzConfirmModal`); Rust only ever sees
/// the confirmed outcome.
///
/// `folders_qt::delete_folder` does TWO writes — the folder row and
/// `local_playlists::clear_folder` — because the app keeps `PRAGMA
/// foreign_keys` OFF, so the `ON DELETE SET NULL` never fires for
/// `local_playlists.folder_id`. Nothing else is deleted: the members move back
/// to the root, which is what the confirm copy promises, for both kinds.
pub(crate) fn edit_delete_confirmed() {
    let id = {
        let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
        if !st.open || st.busy || st.id.is_empty() {
            return;
        }
        st.busy = true;
        st.id.clone()
    };
    publish_edit();

    crate::spawn(async move {
        let ok = tokio::task::spawn_blocking(move || folders_qt::delete_folder(&id))
            .await
            .unwrap_or(false);

        {
            let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
            st.busy = false;
            st.open = !ok;
        }
        publish_edit();

        if ok {
            after_write();
        } else {
            log::warn!("[qbz-qt] folder delete failed");
            crate::toast_qt::error(qbz_i18n::t("Failed to delete folder"));
        }
    });
}

/// Dismiss. Refused while a save or delete is in flight (D22).
pub(crate) fn edit_close() {
    {
        let mut st = EDIT.lock().unwrap_or_else(|e| e.into_inner());
        if st.busy {
            return;
        }
        st.open = false;
    }
    publish_edit();
}

/// Sidebar row menu -> "Hide from sidebar" on a FOLDER row. No panel, no
/// `busy`: it is a one-field flip, and `set_folder_hidden` passes `None` for
/// every other column precisely so the image and the colour survive it.
pub(crate) fn set_hidden(folder_id: &str, hidden: bool) {
    let id = folder_id.to_string();
    if id.is_empty() {
        return;
    }
    crate::spawn(async move {
        let _ =
            tokio::task::spawn_blocking(move || folders_qt::set_folder_hidden(&id, hidden)).await;
        after_write();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_swatch_document_marks_exactly_the_empty_value_as_the_accent() {
        let all = SwatchDoc::all();
        assert_eq!(all.len(), 11);
        assert!(all[0].is_accent);
        assert_eq!(all[0].value, "");
        assert!(all[1..].iter().all(|s| !s.is_accent));
    }

    #[test]
    fn the_preset_list_is_the_references_seven_in_order() {
        assert_eq!(
            PRESETS,
            [
                "heart",
                "star",
                "music",
                "folder",
                "disc",
                "library",
                "headphones"
            ]
        );
    }

    #[test]
    fn the_closed_edit_document_parses_and_carries_the_constants() {
        // The default state must serialise to something a QML binding can read
        // on the pre-publish frame.
        let json = edit_doc();
        let v: serde_json::Value = serde_json::from_str(&json).expect("edit doc parses");
        assert_eq!(v["open"], serde_json::Value::Bool(false));
        assert_eq!(v["presets"].as_array().map(|a| a.len()), Some(7));
        assert_eq!(v["swatches"].as_array().map(|a| a.len()), Some(11));
        assert_eq!(v["swatches"][0]["isAccent"], serde_json::Value::Bool(true));
        assert_eq!(
            v["customImagePath"],
            serde_json::Value::String(String::new())
        );
    }
}
