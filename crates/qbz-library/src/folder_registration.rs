//! Canonical, typed registration for Local Library roots.

use crate::{is_network_path, network_fs_label, LibraryDatabase};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum RegisterFolderOutcome {
    Added { folder_id: i64 },
    Refreshed { folder_id: i64 },
    Covered { folder_id: i64 },
    RegisteredDisabled { folder_id: i64 },
    Conflict,
    Failed,
}

impl RegisterFolderOutcome {
    pub fn scan_folder_id(&self) -> Option<i64> {
        match self {
            Self::Added { folder_id }
            | Self::Refreshed { folder_id }
            | Self::Covered { folder_id } => Some(*folder_id),
            Self::RegisteredDisabled { .. } | Self::Conflict | Self::Failed => None,
        }
    }
}

impl LibraryDatabase {
    /// Register a canonical root, or identify the exact existing root that
    /// should be refreshed. Path comparisons are component-wise (`Path`), not
    /// string-prefix comparisons.
    pub fn register_or_refresh_folder(&self, path: &Path) -> RegisterFolderOutcome {
        let Ok(candidate) = path.canonicalize() else {
            return RegisterFolderOutcome::Failed;
        };
        if !candidate.is_dir() {
            return RegisterFolderOutcome::Failed;
        }

        let rows = match self.get_folders_with_metadata() {
            Ok(rows) => rows,
            Err(_) => return RegisterFolderOutcome::Failed,
        };
        let existing: Vec<_> = rows
            .iter()
            .map(|row| (row, PathBuf::from(&row.path)))
            .collect();

        if let Some((row, _)) = existing.iter().find(|(_, root)| root == &candidate) {
            return if row.enabled {
                RegisterFolderOutcome::Refreshed { folder_id: row.id }
            } else {
                RegisterFolderOutcome::RegisteredDisabled { folder_id: row.id }
            };
        }

        if let Some((row, _)) = existing
            .iter()
            .filter(|(_, root)| candidate.starts_with(root))
            .max_by_key(|(_, root)| root.components().count())
        {
            return if row.enabled {
                RegisterFolderOutcome::Covered { folder_id: row.id }
            } else {
                RegisterFolderOutcome::RegisteredDisabled { folder_id: row.id }
            };
        }

        if existing
            .iter()
            .any(|(_, root)| root.starts_with(&candidate))
        {
            return RegisterFolderOutcome::Conflict;
        }

        let canonical = candidate.to_string_lossy().into_owned();
        let is_network = is_network_path(&candidate);
        let fs_label = is_network.then(|| network_fs_label(&candidate)).flatten();
        self.with_connection(|conn| {
            let inserted = match conn.execute(
                "INSERT OR IGNORE INTO library_folders
                 (path, is_network, network_fs_type) VALUES (?1, ?2, ?3)",
                params![canonical, is_network as i32, fs_label],
            ) {
                Ok(count) => count,
                Err(_) => return RegisterFolderOutcome::Failed,
            };
            let row = conn
                .query_row(
                    "SELECT id, enabled FROM library_folders WHERE path = ?1",
                    [canonical],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i32>(1)? != 0)),
                )
                .optional();
            match row {
                Ok(Some((folder_id, _))) if inserted > 0 => {
                    RegisterFolderOutcome::Added { folder_id }
                }
                Ok(Some((folder_id, true))) => RegisterFolderOutcome::Refreshed { folder_id },
                Ok(Some((folder_id, false))) => {
                    RegisterFolderOutcome::RegisteredDisabled { folder_id }
                }
                _ => RegisterFolderOutcome::Failed,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_descendant_paths_reuse_one_root() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        let album = music.join("Artist").join("Album");
        std::fs::create_dir_all(&album).unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();

        let RegisterFolderOutcome::Added { folder_id } = db.register_or_refresh_folder(&music)
        else {
            panic!("first registration must add");
        };
        assert_eq!(
            db.register_or_refresh_folder(&music),
            RegisterFolderOutcome::Refreshed { folder_id }
        );
        assert_eq!(
            db.register_or_refresh_folder(&album),
            RegisterFolderOutcome::Covered { folder_id }
        );
        assert_eq!(db.get_folders_with_metadata().unwrap().len(), 1);
    }

    #[test]
    fn path_prefix_siblings_are_not_ancestors() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        let music_archive = tmp.path().join("music-archive");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&music_archive).unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        assert!(matches!(
            db.register_or_refresh_folder(&music),
            RegisterFolderOutcome::Added { .. }
        ));
        assert!(matches!(
            db.register_or_refresh_folder(&music_archive),
            RegisterFolderOutcome::Added { .. }
        ));
    }

    #[test]
    fn parent_of_an_existing_root_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("music");
        let child = parent.join("collection");
        std::fs::create_dir_all(&child).unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        assert!(matches!(
            db.register_or_refresh_folder(&child),
            RegisterFolderOutcome::Added { .. }
        ));
        assert_eq!(
            db.register_or_refresh_folder(&parent),
            RegisterFolderOutcome::Conflict
        );
    }

    #[test]
    fn disabled_roots_are_never_reenabled_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let db = LibraryDatabase::open(&tmp.path().join("library.db")).unwrap();
        let RegisterFolderOutcome::Added { folder_id } = db.register_or_refresh_folder(&music)
        else {
            panic!("first registration must add");
        };
        db.set_folder_enabled(folder_id, false).unwrap();
        assert_eq!(
            db.register_or_refresh_folder(&music),
            RegisterFolderOutcome::RegisteredDisabled { folder_id }
        );
        assert!(!db.get_folder_by_id(folder_id).unwrap().unwrap().enabled);
    }
}
