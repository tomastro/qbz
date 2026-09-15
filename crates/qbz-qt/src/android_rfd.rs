//! Temporary Android file-dialog boundary.
//!
//! `rfd` has no Android backend. Keeping its tiny call shape here lets the
//! shared Qt/Rust frontend compile while Android's Storage Access Framework
//! picker is connected through QJniObject. Returning `None` has the same
//! semantics as cancelling the native picker, so no caller mutates state or
//! invents a filesystem path in the meantime.

use std::path::{Path, PathBuf};

pub struct AsyncFileDialog;

impl AsyncFileDialog {
    pub const fn new() -> Self {
        Self
    }

    pub fn set_title(self, _title: impl AsRef<str>) -> Self {
        self
    }

    pub fn set_directory(self, _directory: impl AsRef<Path>) -> Self {
        self
    }

    pub fn set_file_name(self, _file_name: impl AsRef<str>) -> Self {
        self
    }

    pub fn add_filter<T: AsRef<str>>(self, _name: impl AsRef<str>, _extensions: &[T]) -> Self {
        self
    }

    pub async fn pick_file(self) -> Option<FileHandle> {
        None
    }

    pub async fn pick_folder(self) -> Option<FileHandle> {
        None
    }

    pub async fn save_file(self) -> Option<FileHandle> {
        None
    }
}

pub struct FileHandle(PathBuf);

impl FileHandle {
    pub fn path(&self) -> &Path {
        &self.0
    }
}
