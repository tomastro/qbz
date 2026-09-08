//! My QBZ — custom-cover upload / remove. The Qt port of
//! `crates/qbz/src/myqbz_cover.rs` (spec 02 §6.5).
//!
//! The STEP ORDER is load-bearing — a reordering silently loses the previous
//! file or orphans the new one:
//!
//! 1. native picker (cancel ⇒ return, NO toast)
//! 2. source must exist; extension ∈ png/jpg/jpeg/webp (lowercased)
//! 3. read the previous `custom_artwork_path` (deleted AFTER the persist)
//! 4. `dest = artwork_cache_dir/mixtape_custom_{safe_id}_{epoch_SECS}.jpg`
//! 5. decode → `resize(1000, 1000, Lanczos3)` → save as JPEG
//! 6. persist the new path; on failure delete the orphan we just wrote
//! 7. delete the previous file ONLY when it differs from dest
//! 8. success toast + reload; on error `log::warn!` + error toast and NO reload
//!
//! NOTE on `.webp`: this crate's `image` is built with `jpeg` + `png` only, so a
//! `.webp` source decodes to an error at runtime. The extension is still
//! ACCEPTED, to match the picker filter, and the decode failure surfaces the
//! "Failed to upload cover" toast. That is the reference's behaviour — do not
//! "fix" it (spec 02 §2.2).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::imageops::FilterType;

/// The four extensions the picker accepts.
const ALLOWED_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Epoch SECONDS, not millis.
fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Keep ascii-alphanumeric / `-` / `_`; everything else becomes `_`. The
/// collection id is a UUID, so this is normally a no-op.
fn safe_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Read the stored `custom_artwork_path` (to delete the previous file after the
/// new one persists). SYNCHRONOUS — called inside `spawn_blocking`.
fn get_prev_path(id: &str) -> Option<String> {
    crate::library_db_qt::with_db(false, |db| {
        Ok(db.with_connection(|conn| {
            qbz_mixtape::repo::get_custom_artwork(conn, id).unwrap_or(None)
        }))
    })
    .flatten()
}

/// Persist `path` (or clear it with `None`). True on success.
fn set_custom_artwork(id: &str, path: Option<&str>) -> bool {
    let id = id.to_string();
    let path = path.map(|p| p.to_string());
    crate::library_db_qt::with_db(true, move |db| {
        db.with_connection(|conn| qbz_mixtape::repo::set_custom_artwork(conn, &id, path.as_deref()))
            .map_err(|e| {
                qbz_library::LibraryError::Database(format!("set_custom_artwork failed: {e}"))
            })
    })
    .is_some()
}

/// Decode, resize to 1000×1000 (Lanczos3) and save as a JPEG.
fn resize_and_save(source: &Path, dest: &Path) -> Result<(), String> {
    let img = image::open(source).map_err(|e| format!("decode failed: {e}"))?;
    let resized = img.resize(1000, 1000, FilterType::Lanczos3);
    resized
        .to_rgb8()
        .save_with_format(dest, image::ImageFormat::Jpeg)
        .map_err(|e| format!("save failed: {e}"))
}

/// The blocking upload body. Returns the persisted destination path.
fn do_upload(id: &str, source_path: &str) -> Result<String, String> {
    let source = PathBuf::from(source_path);
    if !source.exists() {
        return Err("source file not found".to_string());
    }
    // 1 — extension validation.
    let ext_ok = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| ALLOWED_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return Err("unsupported image type".to_string());
    }

    // 2 — previous path, deleted only AFTER the persist lands.
    let prev = get_prev_path(id);

    // 3 — destination in the shared artwork cache dir.
    let artwork_dir = qbz_library::get_artwork_cache_dir();
    let filename = format!("mixtape_custom_{}_{}.jpg", safe_id(id), epoch_secs());
    let dest = artwork_dir.join(&filename);

    // 4 — decode + resize + save.
    resize_and_save(&source, &dest)?;

    // 5 — persist.
    let dest_str = dest.to_string_lossy().to_string();
    if !set_custom_artwork(id, Some(&dest_str)) {
        // Persist failed — clean up the orphan we just wrote.
        let _ = std::fs::remove_file(&dest);
        return Err("failed to save cover".to_string());
    }

    // 6 — delete the previous file, only if it differs.
    if let Some(prev) = prev {
        if prev != dest_str {
            let _ = std::fs::remove_file(&prev);
        }
    }

    Ok(dest_str)
}

/// The blocking remove body: read previous → clear → delete prev.
fn do_remove(id: &str) -> Result<(), String> {
    let prev = get_prev_path(id);
    if !set_custom_artwork(id, None) {
        return Err("failed to clear cover".to_string());
    }
    if let Some(prev) = prev {
        let _ = std::fs::remove_file(&prev);
    }
    Ok(())
}

/// Reload the open detail so the hero picks up the new cover, and re-list the
/// grids so their mosaic / custom-cover card agrees (spec 02 §7 T4).
fn reload(id: String) {
    crate::myqbz_detail_qt::reload(id);
    crate::myqbz_qt::reload_grids();
}

/// Hero overflow "Set custom cover": native picker, then upload + resize +
/// persist + reload.
pub(crate) fn upload() {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() {
        return;
    }
    crate::spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title(&qbz_i18n::t("Choose a cover image"))
            .add_filter(&qbz_i18n::t("Image"), &["png", "jpg", "jpeg", "webp"])
            .pick_file()
            .await
        else {
            return; // cancelled — no toast.
        };
        let source = file.path().to_string_lossy().to_string();

        let upload_id = id.clone();
        let result = tokio::task::spawn_blocking(move || do_upload(&upload_id, &source))
            .await
            .unwrap_or_else(|e| Err(format!("upload task panicked: {e}")));

        match result {
            Ok(_) => {
                crate::toast_qt::success(qbz_i18n::t("Cover updated"));
                reload(id);
            }
            Err(e) => {
                log::warn!("[qbz-qt] myqbz_cover upload failed: {e}");
                crate::toast_qt::error(qbz_i18n::t("Failed to upload cover"));
            }
        }
    });
}

/// Hero overflow "Remove custom cover": clear + delete the file + reload.
pub(crate) fn remove() {
    let id = crate::myqbz_detail_qt::current_id();
    if id.is_empty() {
        return;
    }
    crate::spawn(async move {
        let remove_id = id.clone();
        let result = tokio::task::spawn_blocking(move || do_remove(&remove_id))
            .await
            .unwrap_or_else(|e| Err(format!("remove task panicked: {e}")));

        match result {
            Ok(()) => {
                crate::toast_qt::success(qbz_i18n::t("Cover removed"));
                reload(id);
            }
            Err(e) => {
                log::warn!("[qbz-qt] myqbz_cover remove failed: {e}");
                crate::toast_qt::error(qbz_i18n::t("Failed to remove cover"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_id_keeps_uuid_characters_and_scrubs_the_rest() {
        assert_eq!(
            safe_id("7f3c9a1e-2b4d-4c8f-9e1a-0d5b6c7a8f90"),
            "7f3c9a1e-2b4d-4c8f-9e1a-0d5b6c7a8f90"
        );
        assert_eq!(safe_id("a/b c.d"), "a_b_c_d");
    }

    #[test]
    fn extension_allow_list_is_case_insensitive_and_includes_webp() {
        for ext in ["PNG", "Jpg", "jpeg", "WEBP"] {
            assert!(ALLOWED_EXTENSIONS.contains(&ext.to_lowercase().as_str()));
        }
        assert!(!ALLOWED_EXTENSIONS.contains(&"gif"));
    }
}
