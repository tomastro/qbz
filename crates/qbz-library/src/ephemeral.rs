//! In-memory ephemeral library for ad-hoc folder playback.
//!
//! The user can point QBZ at a folder that lives outside their library
//! (a downloaded album they haven't decided to keep, an external drive,
//! etc.), browse it, and play tracks from it without anything landing
//! in `local_tracks`. The ephemeral session lives only in memory: a
//! `HashMap<i64, LocalTrack>` keyed by *synthetic ids in the high
//! range* (>= `EPHEMERAL_ID_FLOOR` = 2^48). Synthetic ids in this range
//! are how the rest of the playback pipeline distinguishes ephemeral
//! tracks from DB-resolvable ones — local_tracks autoincrement IDs are
//! orders of magnitude smaller, so any track_id arriving at
//! `v2_library_play_track` at or above the floor is unambiguously
//! ephemeral and gets routed here instead of the DB.
//!
//! The high-positive design (instead of the obvious "use negatives")
//! exists because the queue/playback-context commands serialize ids as
//! `u64` end-to-end (V2QueueTrack, v2_set_playback_context) and reject
//! negative numbers at the serde boundary. Positive ids above the DB
//! range and below 2^53 (JS Number safe limit) are valid u64 *and*
//! survive the JSON round-trip without precision loss.
//!
//! Only one folder is held at a time; opening a new folder replaces the
//! previous session. The state vanishes on app exit by virtue of being
//! in-memory — nothing persists, no migration, no cleanup logic needed.
//!
//! This module is frontend-agnostic (ADR-006): it has zero Tauri/Slint
//! dependency and is consumed by both frontends. The Tauri build re-exports
//! it via `src-tauri/src/ephemeral_library/mod.rs`; the Slint build wraps it
//! in a process-global singleton in `crates/qbz-slint/src/ephemeral.rs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{
    cue_to_tracks, AlbumTagSidecar, CueParser, LibraryError, LibraryScanner, LocalTrack,
    MetadataExtractor,
};
use serde::Serialize;

fn should_expand_cue(cue: &crate::CueSheet) -> bool {
    cue.is_single_file_image()
}

fn apply_sidecar_override(
    track: &mut LocalTrack,
    cache: &mut HashMap<PathBuf, Option<AlbumTagSidecar>>,
) {
    let own_directory = Path::new(&track.file_path).parent().map(Path::to_path_buf);
    let grouped_directory = (!track.album_group_key.trim().is_empty())
        .then(|| PathBuf::from(&track.album_group_key))
        .filter(|path| path.is_dir());
    for directory in grouped_directory.into_iter().chain(own_directory) {
        let sidecar = cache
            .entry(directory.clone())
            .or_insert_with(|| crate::read_album_sidecar(&directory).unwrap_or(None));
        if let Some(sidecar) = sidecar.as_ref() {
            crate::apply_sidecar_to_track(track, sidecar);
            return;
        }
    }
}

/// Floor for synthetic ephemeral track ids. Any id at or above this
/// value is an ephemeral track; below it is a DB row id. Set high
/// enough to be impossible to collide with autoincrement DB ids in any
/// realistic library size, low enough to fit in JS Number's safe
/// integer range (2^53 - 1) so the JSON round-trip stays lossless.
pub const EPHEMERAL_ID_FLOOR: i64 = 1 << 48;

#[derive(Debug, Serialize, Clone)]
pub struct EphemeralFolderResult {
    pub folder_path: String,
    pub tracks: Vec<LocalTrack>,
    pub skipped_files: usize,
}

#[derive(Debug)]
pub enum EphemeralError {
    Lock,
    Library(String),
    Io(String),
}

impl std::fmt::Display for EphemeralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lock => write!(f, "ephemeral library state lock poisoned"),
            Self::Library(msg) => write!(f, "{}", msg),
            Self::Io(msg) => write!(f, "{}", msg),
        }
    }
}

impl From<LibraryError> for EphemeralError {
    fn from(e: LibraryError) -> Self {
        EphemeralError::Library(e.to_string())
    }
}

struct EphemeralLibraryInner {
    tracks: HashMap<i64, LocalTrack>,
    next_id: i64,
    current_folder_path: Option<String>,
}

impl EphemeralLibraryInner {
    fn new() -> Self {
        Self {
            tracks: HashMap::new(),
            next_id: EPHEMERAL_ID_FLOOR,
            current_folder_path: None,
        }
    }

    fn reset(&mut self) {
        self.tracks.clear();
        self.next_id = EPHEMERAL_ID_FLOOR;
        self.current_folder_path = None;
    }
}

pub struct EphemeralLibraryState {
    inner: Mutex<EphemeralLibraryInner>,
}

impl EphemeralLibraryState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(EphemeralLibraryInner::new()),
        }
    }

    /// Scan a folder, extract metadata for every supported audio file
    /// found, assign synthetic high ids and stash the result. The
    /// previous ephemeral session, if any, is dropped.
    pub fn open_folder(&self, path: &Path) -> Result<EphemeralFolderResult, EphemeralError> {
        if !path.exists() {
            return Err(EphemeralError::Io(format!(
                "Folder does not exist: {}",
                path.display()
            )));
        }
        if !path.is_dir() {
            return Err(EphemeralError::Io(format!(
                "Not a directory: {}",
                path.display()
            )));
        }

        let scanner = LibraryScanner::new();
        let scan = scanner.scan_directory(path)?;

        let mut tracks_out: Vec<LocalTrack> = Vec::with_capacity(scan.audio_files.len());
        let mut skipped_files: usize = 0;

        let mut inner = self.inner.lock().map_err(|_| EphemeralError::Lock)?;
        inner.reset();

        // Cache directory for artwork thumbnails. Same one the regular
        // index uses, so ephemeral artwork piggy-backs on the existing
        // thumbnail pipeline (and gets evicted by the same housekeeping).
        let artwork_cache = crate::get_artwork_cache_dir();

        // Two artwork caches keyed at different granularities. The bigger
        // win is the album-level cache: embedded covers are usually
        // identical across every track of an album, so doing extract_artwork
        // (Probe::open + thumbnail encode) 155 times for a 155-track album
        // is wasted I/O. The folder-level cache is a smaller secondary
        // saver for find_folder_artwork (cover.jpg lookup) when albums
        // share the same parent directory.
        let mut album_artwork_cache: HashMap<String, Option<String>> = HashMap::new();
        let mut folder_artwork_cache: HashMap<PathBuf, Option<String>> = HashMap::new();
        let mut sidecar_cache: HashMap<PathBuf, Option<AlbumTagSidecar>> = HashMap::new();

        // Audio files referenced by CUE sheets. We index those audio files
        // through the CUE path (one logical "album" file gets exploded
        // into N virtual tracks) and skip them in the regular audio loop
        // below — otherwise the user would see both the CUE-derived
        // tracks and a single-row entry for the underlying FLAC/WAV.
        let mut cue_referenced_audio: HashSet<PathBuf> = HashSet::new();

        for cue_path in &scan.cue_files {
            match CueParser::parse(cue_path) {
                Ok(mut cue) => {
                    // A multi-file CUE is a sidecar for audio that has
                    // already been split (usually one FILE per TRACK). The
                    // regular audio loop below is authoritative in that
                    // layout. Expanding it as though it were one image would
                    // duplicate every row and bind the virtual entries to an
                    // arbitrary backing file.
                    if !should_expand_cue(&cue) {
                        log::info!(
                            "[ephemeral] treating multi-file CUE as sidecar ({} files): {}",
                            cue.audio_file_count(),
                            cue_path.display()
                        );
                        continue;
                    }
                    let audio_path_raw = Path::new(&cue.audio_file).to_path_buf();
                    let canonical = std::fs::canonicalize(&audio_path_raw)
                        .unwrap_or_else(|_| audio_path_raw.clone());
                    if !canonical.exists() {
                        log::warn!(
                            "[ephemeral] CUE references missing audio: {} -> {}",
                            cue_path.display(),
                            audio_path_raw.display()
                        );
                        skipped_files += 1;
                        continue;
                    }
                    cue.audio_file = canonical.to_string_lossy().to_string();

                    // The decoder behind play_data is Symphonia, which
                    // covers FLAC / MP3 / M4A (AAC + ALAC) / ALAC /
                    // WAV / AIFF out of the box (`features = ["all"]`).
                    // APE (Monkey's Audio) and raw BIN (CD-DA dumps
                    // without headers) aren't in that list: playback
                    // either errors out or produces white noise as
                    // Symphonia mis-probes the stream. Skip CUE files
                    // that point at those — better an empty pane than
                    // a track row that explodes on click.
                    let ext_lower = canonical
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_lowercase());
                    let playable_via_cue = matches!(
                        ext_lower.as_deref(),
                        Some("flac" | "mp3" | "m4a" | "alac" | "wav" | "aiff" | "aif")
                    );
                    if !playable_via_cue {
                        log::warn!(
                            "[ephemeral] CUE references unsupported audio format ({:?}) — skipping: {}",
                            ext_lower,
                            canonical.display()
                        );
                        skipped_files += 1;
                        continue;
                    }

                    let properties = match MetadataExtractor::extract_properties(&canonical) {
                        Ok(p) => p,
                        Err(e) => {
                            log::warn!(
                                "[ephemeral] failed to read audio properties for {}: {}",
                                canonical.display(),
                                e
                            );
                            skipped_files += 1;
                            continue;
                        }
                    };
                    let format = MetadataExtractor::detect_format(&canonical);

                    let mut cue_tracks =
                        cue_to_tracks(&cue, properties.duration_secs, format, &properties);
                    if cue_tracks.is_empty() {
                        log::warn!("[ephemeral] CUE produced no tracks");
                        skipped_files += 1;
                        continue;
                    }

                    // CUE = single album: resolve cover once, share across
                    // every CUE-derived track. Use a key derived from the
                    // CUE path so the cache survives even when the
                    // album_group_key field is empty (rare but possible
                    // for CUE files without explicit TITLE/PERFORMER).
                    let album_key = if !cue_tracks[0].album_group_key.is_empty() {
                        cue_tracks[0].album_group_key.clone()
                    } else {
                        format!("cue:{}", cue.file_path)
                    };
                    let artwork = if let Some(cached) = album_artwork_cache.get(&album_key) {
                        cached.clone()
                    } else {
                        let mut found =
                            MetadataExtractor::extract_artwork(&canonical, &artwork_cache);
                        if found.is_none() {
                            if let Some(folder_art) = MetadataExtractor::find_folder_artwork(
                                &canonical,
                                cue.title.as_deref(),
                            ) {
                                found = MetadataExtractor::cache_artwork_file(
                                    Path::new(&folder_art),
                                    &artwork_cache,
                                );
                            }
                        }
                        album_artwork_cache.insert(album_key, found.clone());
                        found
                    };

                    for mut track in cue_tracks.drain(..) {
                        apply_sidecar_override(&mut track, &mut sidecar_cache);
                        track.id = inner.next_id;
                        inner.next_id += 1;
                        track.source = Some("ephemeral".to_string());
                        track.artwork_path = artwork.clone();
                        inner.tracks.insert(track.id, track.clone());
                        tracks_out.push(track);
                    }
                    cue_referenced_audio.insert(canonical);
                }
                Err(e) => {
                    log::warn!("[ephemeral] failed to parse CUE: {}", e);
                    skipped_files += 1;
                }
            }
        }

        // ── THE FILE READS RUN IN PARALLEL; EVERYTHING ELSE STAYS SERIAL ──
        //
        // Opening a folder of 247 FLACs on a NAS took ~15 s between "Scanned"
        // and "ephemeral opened" (owner's log, 2026-08-22), on what is meant to
        // be the QUICK way to play something outside the library. The cost is
        // not computation: it is 247 sequential `canonicalize` + tag-read round
        // trips over the network, each a few tens of milliseconds, one after
        // another.
        //
        // So only the READS move. The bookkeeping below — the CUE-dedup check,
        // the APE skip, id assignment, the artwork caches, the push order — is
        // untouched and still runs in scan order, because ids and ordering are
        // observable and a parallel loop must not renumber anything.
        //
        // `std::thread::scope`, not a new dependency: this is I/O-bound, the
        // work items are independent, and borrowing `scan.audio_files` across
        // the scope needs no Arc.
        let t_probe = std::time::Instant::now();
        let probed: Vec<(std::path::PathBuf, Result<LocalTrack, LibraryError>)> = {
            let files: &[std::path::PathBuf] = &scan.audio_files;
            let workers = std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(4)
                .max(1);
            let chunk = files.len().div_ceil(workers).max(1);
            let mut out: Vec<Vec<(std::path::PathBuf, Result<LocalTrack, LibraryError>)>> =
                Vec::new();
            std::thread::scope(|sc| {
                let handles: Vec<_> = files
                    .chunks(chunk)
                    .map(|part| {
                        sc.spawn(move || {
                            part.iter()
                                .map(|f| {
                                    let canonical =
                                        std::fs::canonicalize(f).unwrap_or_else(|_| f.clone());
                                    (canonical, MetadataExtractor::extract(f))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                for h in handles {
                    // A panicking worker must not take the whole open down: the
                    // chunk is dropped and its files are simply absent, which
                    // the counters below already report as skipped.
                    out.push(h.join().unwrap_or_default());
                }
            });
            out.into_iter().flatten().collect()
        };

        log::info!(
            "[ephemeral][perf] parallel tag reads: {:?} for {} files",
            t_probe.elapsed(),
            probed.len()
        );
        let t_rest = std::time::Instant::now();
        for (audio_file, (canonical_audio, extracted)) in
            scan.audio_files.iter().zip(probed.into_iter())
        {
            // Skip audio files that were already exploded into tracks via
            // a CUE sheet — listing them again as a single row would
            // duplicate the album and confuse playback (the CUE-derived
            // track ids are the canonical ones).
            if cue_referenced_audio.contains(&canonical_audio) {
                continue;
            }

            // The scanner accepts APE because the regular library tracks
            // them for tag/metadata purposes, but Symphonia can't decode
            // Monkey's Audio. In ephemeral mode there is no value in
            // surfacing rows that explode on click — skip them so the
            // pane only shows tracks the user can actually play.
            let ext_lower = audio_file
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase());
            if matches!(ext_lower.as_deref(), Some("ape")) {
                log::info!(
                    "[ephemeral] skipping APE (no Symphonia decoder): {}",
                    audio_file.display()
                );
                skipped_files += 1;
                continue;
            }
            match extracted {
                Ok(mut track) => {
                    apply_sidecar_override(&mut track, &mut sidecar_cache);
                    track.id = inner.next_id;
                    inner.next_id += 1;
                    track.source = Some("ephemeral".to_string());

                    let album_key = if !track.album_group_key.is_empty() {
                        track.album_group_key.clone()
                    } else {
                        format!(
                            "{}|||{}",
                            track.album,
                            track.album_artist.as_deref().unwrap_or(&track.artist)
                        )
                    };

                    let artwork = if let Some(cached) = album_artwork_cache.get(&album_key) {
                        cached.clone()
                    } else {
                        let mut found =
                            MetadataExtractor::extract_artwork(audio_file, &artwork_cache);
                        if found.is_none() {
                            let folder_key = audio_file
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| audio_file.to_path_buf());
                            let folder_art = folder_artwork_cache
                                .entry(folder_key)
                                .or_insert_with(|| {
                                    MetadataExtractor::find_folder_artwork(
                                        audio_file,
                                        Some(track.album.as_str()),
                                    )
                                })
                                .clone();
                            if let Some(folder_art) = folder_art {
                                found = MetadataExtractor::cache_artwork_file(
                                    std::path::Path::new(&folder_art),
                                    &artwork_cache,
                                );
                            }
                        }
                        album_artwork_cache.insert(album_key, found.clone());
                        found
                    };
                    track.artwork_path = artwork;

                    inner.tracks.insert(track.id, track.clone());
                    tracks_out.push(track);
                }
                Err(e) => {
                    log::warn!(
                        "[ephemeral] failed to extract metadata from {}: {}",
                        audio_file.display(),
                        e
                    );
                    skipped_files += 1;
                }
            }
        }

        log::info!(
            "[ephemeral][perf] serial bookkeeping (artwork + ids): {:?}",
            t_rest.elapsed()
        );

        // Musical order (album, then disc/track/title — same as the DB-backed
        // folder view): the extraction order above is readdir order, which is
        // arbitrary. Ids must FOLLOW the display order because
        // `tracks_snapshot` builds play queues by id — so this call's entries
        // are re-keyed after sorting.
        tracks_out.sort_by(|a, b| {
            a.album_group_key
                .cmp(&b.album_group_key)
                .then_with(|| a.disc_number.unwrap_or(1).cmp(&b.disc_number.unwrap_or(1)))
                .then_with(|| {
                    a.track_number
                        .unwrap_or(u32::MAX)
                        .cmp(&b.track_number.unwrap_or(u32::MAX))
                })
                .then_with(|| a.title.cmp(&b.title))
        });
        for track in &tracks_out {
            inner.tracks.remove(&track.id);
        }
        for track in &mut tracks_out {
            track.id = inner.next_id;
            inner.next_id += 1;
            inner.tracks.insert(track.id, track.clone());
        }

        let folder_path = path.display().to_string();
        inner.current_folder_path = Some(folder_path.clone());

        log::info!(
            "[ephemeral] opened {} ({} tracks, {} skipped)",
            folder_path,
            tracks_out.len(),
            skipped_files
        );

        Ok(EphemeralFolderResult {
            folder_path,
            tracks: tracks_out,
            skipped_files,
        })
    }

    /// Install a track list that did NOT come from scanning a directory — a
    /// CD in the drive, and later a disc image.
    ///
    /// `open_folder` cannot serve these: it takes a `&Path`, rejects anything
    /// that is not a real directory (`:118`), and derives every field by
    /// reading files off a filesystem. A disc has no filesystem to read.
    ///
    /// `label` is what the medium is CALLED (the album title, or "Audio CD"),
    /// and it is stored where a folder path would be, because everything
    /// downstream — the pane header, the tab, the persisted-session check —
    /// asks the session what it is, not where it lives.
    ///
    /// Ids are assigned from the SAME synthetic range as a folder session, so
    /// every playback caller keeps routing them to this store without knowing
    /// a disc exists.
    pub fn open_tracks(
        &self,
        label: &str,
        tracks: Vec<LocalTrack>,
    ) -> Result<EphemeralFolderResult, EphemeralError> {
        let mut inner = self.inner.lock().map_err(|_| EphemeralError::Lock)?;
        inner.reset();
        let mut out = Vec::with_capacity(tracks.len());
        for mut track in tracks {
            track.id = inner.next_id;
            inner.next_id += 1;
            track.source = Some("ephemeral".to_string());
            inner.tracks.insert(track.id, track.clone());
            out.push(track);
        }
        inner.current_folder_path = Some(label.to_string());
        Ok(EphemeralFolderResult {
            folder_path: label.to_string(),
            tracks: out,
            skipped_files: 0,
        })
    }

    /// Swap the stored rows for an updated copy, KEEPING their ids.
    ///
    /// Used when something about the session changes after it opened — the
    /// cover arriving late is the case that motivated it. Re-running
    /// `open_tracks` would renumber every row from the floor, and the queue
    /// may already hold the old ids: a playing track would lose its source
    /// mid-song.
    pub fn replace_tracks_preserving_ids(
        &self,
        tracks: &[LocalTrack],
    ) -> Result<(), EphemeralError> {
        let mut inner = self.inner.lock().map_err(|_| EphemeralError::Lock)?;
        for t in tracks {
            // Only rows this session already knows; an unknown id here means
            // the caller built its list from something else.
            if inner.tracks.contains_key(&t.id) {
                inner.tracks.insert(t.id, t.clone());
            }
        }
        Ok(())
    }

    /// Replace a bounded subset only while the same folder/disc session is
    /// still open. The exact id/path check prevents a draft from a dismissed
    /// editor from mutating a later session that reused the synthetic ids.
    pub fn replace_tracks_for_session(
        &self,
        expected_folder_path: &str,
        tracks: &[LocalTrack],
    ) -> Result<Option<Vec<LocalTrack>>, EphemeralError> {
        let mut inner = self.inner.lock().map_err(|_| EphemeralError::Lock)?;
        if inner.current_folder_path.as_deref() != Some(expected_folder_path) {
            return Ok(None);
        }
        for track in tracks {
            let Some(current) = inner.tracks.get(&track.id) else {
                return Ok(None);
            };
            if current.file_path != track.file_path
                || current.cue_start_secs != track.cue_start_secs
            {
                return Ok(None);
            }
        }
        for track in tracks {
            inner.tracks.insert(track.id, track.clone());
        }
        let mut snapshot = inner.tracks.values().cloned().collect::<Vec<_>>();
        snapshot.sort_by_key(|track| track.id);
        Ok(Some(snapshot))
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.reset();
        }
    }

    /// Resolve a synthetic high id to the cached `LocalTrack`. Returns
    /// `None` if the id is unknown (stale queue entry from a previous
    /// session, race against `clear`, etc.).
    pub fn get_track(&self, id: i64) -> Option<LocalTrack> {
        let inner = self.inner.lock().ok()?;
        inner.tracks.get(&id).cloned()
    }

    /// Snapshot of every track in the current session, in stable id order
    /// (insertion order = scan order). Used by the Slint UI to build a
    /// queue from the whole folder or a single album group.
    pub fn tracks_snapshot(&self) -> Vec<LocalTrack> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let mut tracks: Vec<LocalTrack> = inner.tracks.values().cloned().collect();
        tracks.sort_by_key(|t| t.id);
        tracks
    }

    /// The path of the currently-open ephemeral folder, if any.
    pub fn current_folder_path(&self) -> Option<String> {
        self.inner.lock().ok()?.current_folder_path.clone()
    }
}

impl Default for EphemeralLibraryState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn track_at(path: &Path) -> LocalTrack {
        LocalTrack {
            file_path: path.to_string_lossy().into_owned(),
            title: "Before".to_string(),
            album: "Before album".to_string(),
            album_group_title: "Before album".to_string(),
            artist: "Artist".to_string(),
            ..LocalTrack::default()
        }
    }

    #[test]
    fn ephemeral_loader_only_expands_single_image_cues() {
        let temp = tempfile::tempdir().unwrap();
        let single_path = temp.path().join("single.cue");
        fs::write(
            &single_path,
            "FILE \"album.flac\" WAVE\n\
               TRACK 01 AUDIO\n\
                 INDEX 01 00:00:00\n\
               TRACK 02 AUDIO\n\
                 INDEX 01 03:00:00\n",
        )
        .unwrap();
        let multi_path = temp.path().join("split.cue");
        fs::write(
            &multi_path,
            "FILE \"01. First.flac\" WAVE\n\
               TRACK 01 AUDIO\n\
                 INDEX 01 00:00:00\n\
             FILE \"02. Second.flac\" WAVE\n\
               TRACK 02 AUDIO\n\
                 INDEX 01 00:00:00\n",
        )
        .unwrap();

        assert!(should_expand_cue(&CueParser::parse(&single_path).unwrap()));
        assert!(!should_expand_cue(&CueParser::parse(&multi_path).unwrap()));
    }

    #[test]
    fn sidecar_overrides_are_visible_to_ephemeral_scans() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("01.flac");
        let sidecar = crate::AlbumTagSidecar::new(
            crate::AlbumMetadataOverride {
                album_title: Some("After album".to_string()),
                album_artist: Some("After artist".to_string()),
                ..crate::AlbumMetadataOverride::default()
            },
            vec![crate::TrackMetadataOverride {
                file_path: file.to_string_lossy().into_owned(),
                cue_start_secs: None,
                title: Some("After".to_string()),
                disc_number: Some(1),
                track_number: Some(1),
            }],
        );
        crate::write_album_sidecar(temp.path(), &sidecar).unwrap();

        let mut track = track_at(&file);
        let mut cache = HashMap::new();
        apply_sidecar_override(&mut track, &mut cache);

        assert_eq!(track.album, "After album");
        assert_eq!(track.album_artist.as_deref(), Some("After artist"));
        assert_eq!(track.title, "After");
    }

    #[test]
    fn stale_editor_snapshot_cannot_retarget_a_new_ephemeral_session() {
        let state = EphemeralLibraryState::new();
        let first = state
            .open_tracks("first", vec![track_at(Path::new("/music/first.flac"))])
            .unwrap();
        let mut edited = first.tracks;
        edited[0].title = "Edited".to_string();
        assert!(state
            .replace_tracks_for_session("first", &edited)
            .unwrap()
            .is_some());

        state
            .open_tracks("second", vec![track_at(Path::new("/music/second.flac"))])
            .unwrap();
        assert!(state
            .replace_tracks_for_session("first", &edited)
            .unwrap()
            .is_none());
        assert_eq!(state.tracks_snapshot()[0].title, "Before");
    }
}
