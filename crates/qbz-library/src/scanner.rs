//! Filesystem scanner for audio files

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::LibraryError;

/// Supported audio file extensions
const SUPPORTED_AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "wav", "aiff", "aif", "ape", "mp3", "dsf", "dff",
];

/// CUE file extension
const CUE_EXTENSION: &str = "cue";

/// Explicit symlink policy for a root. The compatibility default follows
/// links, but WalkDir's loop detector remains enabled and every loop/error is
/// surfaced to the caller; there is no silent `filter_map(Result::ok)` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    Ignore,
    FollowWithCycleDetection,
}

impl Default for SymlinkPolicy {
    fn default() -> Self {
        Self::FollowWithCycleDetection
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFileKind {
    Audio,
    Cue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub kind: ScanFileKind,
}

/// A traversal error tied to the path/subtree WalkDir could not enumerate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanWalkError {
    pub path: Option<PathBuf>,
    pub subtree: PathBuf,
    pub message: String,
}

pub struct ScanStream {
    root: PathBuf,
    inner: walkdir::IntoIter,
}

impl Iterator for ScanStream {
    type Item = Result<ScanEntry, ScanWalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = match self.inner.next()? {
                Ok(entry) => entry,
                Err(error) => {
                    let path = error.path().map(Path::to_path_buf);
                    let subtree = error
                        .loop_ancestor()
                        .map(Path::to_path_buf)
                        .or_else(|| path.clone())
                        .unwrap_or_else(|| self.root.clone());
                    return Some(Err(ScanWalkError {
                        path,
                        subtree,
                        message: error.to_string(),
                    }));
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let Some(ext) = entry.path().extension().and_then(|value| value.to_str()) else {
                continue;
            };
            let kind = if LibraryScanner::is_supported_audio_extension(&ext.to_lowercase()) {
                ScanFileKind::Audio
            } else if ext.eq_ignore_ascii_case(CUE_EXTENSION) {
                ScanFileKind::Cue
            } else {
                continue;
            };
            return Some(Ok(ScanEntry {
                path: entry.into_path(),
                kind,
            }));
        }
    }
}

/// Result of scanning a directory
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Audio files found
    pub audio_files: Vec<PathBuf>,
    /// CUE files found
    pub cue_files: Vec<PathBuf>,
}

/// Library scanner for discovering audio files
pub struct LibraryScanner;

impl LibraryScanner {
    /// Create a new scanner
    pub fn new() -> Self {
        Self
    }

    /// Enumerate one root lazily in deterministic depth-first order.
    pub fn stream_directory(&self, path: &Path) -> Result<ScanStream, LibraryError> {
        self.stream_directory_with_policy(path, SymlinkPolicy::default())
    }

    pub fn stream_directory_with_policy(
        &self,
        path: &Path,
        policy: SymlinkPolicy,
    ) -> Result<ScanStream, LibraryError> {
        if !path.exists() {
            return Err(LibraryError::InvalidPath(format!(
                "Path does not exist: {}",
                path.display()
            )));
        }
        if !path.is_dir() {
            return Err(LibraryError::InvalidPath(format!(
                "Path is not a directory: {}",
                path.display()
            )));
        }
        let follow = policy == SymlinkPolicy::FollowWithCycleDetection;
        Ok(ScanStream {
            root: path.to_path_buf(),
            inner: WalkDir::new(path)
                .follow_links(follow)
                .sort_by_file_name()
                .into_iter(),
        })
    }

    /// Scan a directory recursively for audio and CUE files
    pub fn scan_directory(&self, path: &Path) -> Result<ScanResult, LibraryError> {
        if !path.exists() {
            return Err(LibraryError::InvalidPath(format!(
                "Path does not exist: {}",
                path.display()
            )));
        }

        if !path.is_dir() {
            return Err(LibraryError::InvalidPath(format!(
                "Path is not a directory: {}",
                path.display()
            )));
        }

        let mut result = ScanResult::default();
        for entry in self.stream_directory(path)? {
            let entry = entry.map_err(|error| LibraryError::Other(error.message))?;
            match entry.kind {
                ScanFileKind::Audio => result.audio_files.push(entry.path),
                ScanFileKind::Cue => result.cue_files.push(entry.path),
            }
        }

        log::info!(
            "Filesystem scan complete: {} audio files, {} CUE files",
            result.audio_files.len(),
            result.cue_files.len()
        );

        Ok(result)
    }

    /// Check if an extension is a supported audio format
    fn is_supported_audio_extension(ext: &str) -> bool {
        SUPPORTED_AUDIO_EXTENSIONS.contains(&ext)
    }

    /// Get all supported extensions (for UI display)
    pub fn supported_extensions() -> &'static [&'static str] {
        SUPPORTED_AUDIO_EXTENSIONS
    }
}

impl Default for LibraryScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_extensions() {
        assert!(LibraryScanner::is_supported_audio_extension("flac"));
        assert!(LibraryScanner::is_supported_audio_extension("wav"));
        assert!(LibraryScanner::is_supported_audio_extension("m4a"));
        assert!(LibraryScanner::is_supported_audio_extension("mp3"));
        assert!(!LibraryScanner::is_supported_audio_extension("txt"));
    }
}
