//! SACD images in the library: one builder turns a parsed stereo area into
//! the virtual `sacd:<image>#<n>` rows, and one pass finds the images a
//! scanned root holds.
//!
//! The folder scan never treats an `.iso` as audio. It keys the walk on the
//! extension, asks [`qbz_disc::sacd::is_sacd_image`] for the `SACDMTOC`
//! signature, and only then parses the Scarlet Book TOC — so a DVD or a data
//! ISO sitting next to the music costs two sector reads and leaves no trace.
//! A disc already imported with the same size and mtime is skipped before
//! any file is opened. Everything that reaches the database goes through the
//! generation-based [`LibraryDatabase::import_sacd_image`], which is what
//! keeps a half-read or NAS-down image from pruning known-good rows.
//!
//! The manual `Open › disc image` path (qbz-qt `sacd_qt`) uses the same
//! builder, with translated fallback labels; the scan uses the English ones,
//! which only ever show for an image whose Master TOC carries no text.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use qbz_disc::sacd::{read_area, SacdError, SacdSniff};

use crate::{AudioFormat, LibraryDatabase, LocalTrack, MetadataExtractor, SacdImageImport};

/// Bump when a successful TOC parse can expose a different complete snapshot.
/// This invalidates only the scan cache; disc identity remains geometry-only.
pub const SACD_PARSER_REVISION: i64 = 2;

/// Fallback naming for an image whose Master TOC carries no text.
#[derive(Debug, Clone)]
pub struct SacdLabels {
    /// Album title when neither the disc nor the file name gives one.
    pub album: String,
    /// Track title pattern; `{}` is replaced by the track number.
    pub track: String,
}

impl Default for SacdLabels {
    fn default() -> Self {
        Self {
            album: "SACD".to_string(),
            track: "Track {}".to_string(),
        }
    }
}

/// What the filesystem says about the image file itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageFacts {
    pub size_bytes: u64,
    pub last_modified: i64,
    pub modified_ns: i64,
    pub is_network_mount: bool,
}

pub fn image_facts(path: &Path) -> ImageFacts {
    let metadata = std::fs::metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    ImageFacts {
        size_bytes: metadata
            .as_ref()
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        last_modified: modified
            .as_ref()
            .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
            .unwrap_or(0),
        modified_ns: modified
            .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
            .unwrap_or(0),
        is_network_mount: crate::is_network_path(path),
    }
}

/// Wall-clock nanoseconds made strictly monotonic inside this process. The DB
/// compares this token while holding an IMMEDIATE transaction, so two
/// imports finishing out of order cannot publish old rows over newer ones. A
/// fresh process naturally starts above its prior token.
pub fn observation_token() -> i64 {
    static LAST: AtomicI64 = AtomicI64::new(0);
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(1);
    let mut seen = LAST.load(Ordering::Acquire);
    loop {
        let next = wall.max(seen.saturating_add(1));
        match LAST.compare_exchange_weak(seen, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return next,
            Err(actual) => seen = actual,
        }
    }
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

/// A parsed image, ready for the catalogue and for an ephemeral session.
#[derive(Debug, Clone)]
pub struct SacdImageRows {
    pub fingerprint: String,
    pub album: String,
    pub artist: Option<String>,
    pub total_playtime_secs: f64,
    pub tracks: Vec<LocalTrack>,
    pub import: SacdImageImport,
}

/// Parse an image and build its rows. A SACD names itself, so nothing here
/// touches the network; the remembered disc memory (a human correction) wins
/// over the disc's own text, which wins over the file name, which wins over
/// the labels. Cover art is whatever sits beside the image, through the same
/// folder walk a scanned album uses.
pub fn build_image_rows(path: &Path, labels: &SacdLabels) -> Result<SacdImageRows, SacdError> {
    let area = read_area(path)?;
    let fingerprint = area.fingerprint();

    let remembered = qbz_disc::store::get(&fingerprint).filter(|memory| memory.edited);
    if remembered.is_some() {
        log::info!("[sacd] corrected by hand — using the remembered naming");
    }

    let album = match remembered
        .as_ref()
        .filter(|memory| !memory.album.is_empty())
    {
        Some(memory) => memory.album.clone(),
        None => area.album.clone().unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| labels.album.clone())
        }),
    };
    let artist = match remembered
        .as_ref()
        .filter(|memory| !memory.album_artist.is_empty())
    {
        Some(memory) => Some(memory.album_artist.clone()),
        None => area.artist.clone().filter(|artist| !artist.is_empty()),
    };

    let artwork = MetadataExtractor::find_folder_artwork(path, Some(&album)).and_then(|found| {
        MetadataExtractor::cache_artwork_file(Path::new(&found), &crate::get_artwork_cache_dir())
    });
    match artwork.as_deref() {
        Some(_) => log::info!("[sacd] cover cached"),
        None => log::info!("[sacd] no cover beside the image"),
    }

    let facts = image_facts(path);
    let indexed_at = unix_now_secs();
    let tracks: Vec<LocalTrack> = area
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| LocalTrack {
            // A corrected title wins over the disc's own; the disc's own wins
            // over the number. Indexed defensively — a remembered row can be
            // shorter than the disc if it was written for a different area.
            title: remembered
                .as_ref()
                .and_then(|memory| memory.tracks.get(i))
                .map(|row| row.title.clone())
                .filter(|title| !title.is_empty())
                .or_else(|| track.title.clone().filter(|title| !title.is_empty()))
                .unwrap_or_else(|| labels.track.replace("{}", &track.number.to_string())),
            album: album.clone(),
            album_group_title: album.clone(),
            // Geometry, not mutable naming, is the album identity. This also
            // keeps two same-title discs separate and survives a correction.
            album_group_key: format!("sacd|||{fingerprint}"),
            artist: remembered
                .as_ref()
                .and_then(|memory| memory.tracks.get(i))
                .map(|row| row.artist.clone())
                .filter(|artist| !artist.is_empty())
                .or_else(|| artist.clone())
                .unwrap_or_default(),
            album_artist: artist.clone(),
            track_number: Some(track.number as u32),
            disc_number: Some(1),
            duration_secs: track.duration_secs as u64,
            // DSD64 stereo. `bit_depth` is the format's nominal 1, and the
            // rate is the DSD bit rate — the same shape a .dsf row carries,
            // which is what makes the quality badge read DSD64.
            sample_rate: 2_822_400.0,
            bit_depth: Some(1),
            format: AudioFormat::Dsd,
            artwork_path: artwork.clone(),
            last_modified: facts.last_modified,
            indexed_at,
            source: Some("user".to_string()),
            is_network_mount: facts.is_network_mount,
            file_path: qbz_disc::SacdRef {
                image: path.to_path_buf(),
                track: track.number,
            }
            .to_path_string(),
            ..Default::default()
        })
        .collect();

    // Remember what the DISC said, so the metadata button has a baseline to
    // show and the rip wizard has defaults. `put_auto` will not touch a row a
    // human corrected — that rule lives in the store, not here.
    qbz_disc::store::put_auto(
        &fingerprint,
        None,
        &qbz_disc::store::DiscMemory {
            album: album.clone(),
            album_artist: artist.clone().unwrap_or_default(),
            year: None,
            tracks: tracks
                .iter()
                .enumerate()
                .map(|(i, track)| qbz_disc::store::TrackMemory {
                    number: track.track_number.unwrap_or(i as u32 + 1),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                })
                .collect(),
            release_id: None,
            release_group_id: None,
            cover_path: artwork.clone(),
            edited: false,
        },
    );

    let import = SacdImageImport {
        fingerprint: fingerprint.clone(),
        image_path: path.to_string_lossy().into_owned(),
        image_size_bytes: facts.size_bytes,
        image_modified_ns: facts.modified_ns,
        observed_at: observation_token(),
        parser_revision: SACD_PARSER_REVISION,
        tracks: tracks.clone(),
    };
    Ok(SacdImageRows {
        fingerprint,
        album,
        artist,
        total_playtime_secs: area.total_playtime_secs,
        tracks,
        import,
    })
}

/// What one root's SACD pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SacdScanSummary {
    /// `.iso` files seen.
    pub candidates: u32,
    /// Discs parsed and (re)imported.
    pub imported: u32,
    /// Known discs skipped on size + mtime, or an import the DB called stale.
    pub unchanged: u32,
    /// Files without the `SACDMTOC` signature — silently left alone.
    pub ignored: u32,
    /// Persistent virtual tracks removed after a complete root walk.
    pub removed: u32,
    /// True only when the complete walk reached its ownership prune.
    pub prune_authorized: bool,
    /// Real SACDs the parser or the import rejected: (path, reason).
    pub failed: Vec<(String, String)>,
}

fn is_iso(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("iso"))
}

/// Find and import the SACD images under one root. Runs after the root's
/// audio/cue passes; nothing here touches `local_scan_files`. A complete walk
/// prunes images no longer present under this root, while cancellation or a
/// traversal error preserves the last known-good rows.
pub fn scan_root_for_sacd(
    db: &LibraryDatabase,
    root_id: i64,
    observed_generation: i64,
    root: &Path,
    labels: &SacdLabels,
    cancel: &AtomicBool,
) -> SacdScanSummary {
    let mut summary = SacdScanSummary::default();
    let mut traversal_complete = true;
    let walk = walkdir::WalkDir::new(root)
        .follow_links(true)
        .sort_by_file_name()
        .into_iter();
    for entry in walk {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::debug!("[sacd] scan walk error: {error}");
                traversal_complete = false;
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_iso(entry.path()) {
            continue;
        }
        summary.candidates = summary.candidates.saturating_add(1);
        let path = entry.path();
        let path_string = path.to_string_lossy().into_owned();

        // Known and unchanged: no file I/O at all.
        let facts = image_facts(path);
        match db.observe_sacd_image(
            root_id,
            observed_generation,
            &path_string,
            facts.size_bytes,
            facts.modified_ns,
            SACD_PARSER_REVISION,
        ) {
            Ok(true) => {
                summary.unchanged = summary.unchanged.saturating_add(1);
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                traversal_complete = false;
                summary.failed.push((path_string, error.to_string()));
                continue;
            }
        }

        // Signature first: a DVD or a data ISO leaves here silently.
        match qbz_disc::sacd::sniff_sacd_image(path) {
            Ok(SacdSniff::Sacd) => {}
            Ok(SacdSniff::NotSacd) => {
                summary.ignored = summary.ignored.saturating_add(1);
                match db.remove_sacd_image_at_path(&path_string) {
                    Ok(removed) => {
                        summary.removed = summary
                            .removed
                            .saturating_add(removed.min(u32::MAX as usize) as u32);
                    }
                    Err(error) => {
                        traversal_complete = false;
                        summary.failed.push((path_string, error.to_string()));
                    }
                }
                continue;
            }
            Err(error) => {
                log::warn!("[sacd] scan: signed image has an invalid Master TOC: {error}");
                summary.failed.push((path_string, error.to_string()));
                continue;
            }
        }

        match build_image_rows(path, labels) {
            Ok(rows) => match db.import_sacd_image(root_id, observed_generation, &rows.import) {
                Ok(result) if result.stale => {
                    summary.unchanged = summary.unchanged.saturating_add(1);
                }
                Ok(result) => {
                    summary.imported = summary.imported.saturating_add(1);
                    log::info!(
                        "[sacd] scan adopted {:?}: tracks={} inserted={} updated={} removed={}",
                        rows.album,
                        rows.tracks.len(),
                        result.inserted,
                        result.updated,
                        result.removed
                    );
                }
                Err(error) => summary.failed.push((path_string, error.to_string())),
            },
            Err(error) => {
                log::warn!("[sacd] scan: image unusable: {error}");
                summary.failed.push((path_string, error.to_string()));
            }
        }
    }
    if !cancel.load(Ordering::Acquire) && traversal_complete {
        match db.finish_sacd_root_scan(root_id, observed_generation) {
            Ok(removed) => {
                summary.removed = summary
                    .removed
                    .saturating_add(removed.min(u32::MAX as usize) as u32);
                summary.prune_authorized = true;
            }
            Err(error) => summary
                .failed
                .push((root.to_string_lossy().into_owned(), error.to_string())),
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::{is_iso, scan_root_for_sacd, SacdLabels, SacdScanSummary};
    use crate::LibraryDatabase;
    use std::io::{Seek, SeekFrom, Write};
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    /// A file that passes the ISO 9660 PVD check but carries no Master TOC:
    /// what a DVD or a data image looks like to the sniff.
    fn write_plain_iso(path: &std::path::Path) {
        let mut file = std::fs::File::create(path).unwrap();
        file.seek(SeekFrom::Start(16 * 2048)).unwrap();
        let mut pvd = [0u8; 2048];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[128] = 0;
        pvd[129] = 8;
        file.write_all(&pvd).unwrap();
        file.seek(SeekFrom::Start(531 * 2048)).unwrap();
        file.write_all(&[0u8; 2048]).unwrap();
    }

    #[test]
    fn extension_match_is_case_insensitive_and_exact() {
        assert!(is_iso(std::path::Path::new("/x/Disc.ISO")));
        assert!(is_iso(std::path::Path::new("/x/disc.iso")));
        assert!(!is_iso(std::path::Path::new("/x/disc.iso.txt")));
        assert!(!is_iso(std::path::Path::new("/x/disc.flac")));
    }

    #[test]
    fn non_sacd_images_are_ignored_without_rows_or_errors() {
        let temp = TempDir::new().unwrap();
        let db = LibraryDatabase::open(&temp.path().join("library.db")).unwrap();
        let root = temp.path().join("music");
        std::fs::create_dir_all(root.join("Some DVD")).unwrap();
        let root_id = db
            .add_folder_with_network_info(&root.to_string_lossy(), false, None)
            .unwrap();
        write_plain_iso(&root.join("Some DVD/movie.iso"));
        std::fs::write(root.join("not-an-image.iso"), b"just bytes").unwrap();
        std::fs::write(root.join("song.flac"), b"").unwrap();

        let summary = scan_root_for_sacd(
            &db,
            root_id,
            1,
            &root,
            &SacdLabels::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(
            summary,
            SacdScanSummary {
                candidates: 2,
                imported: 0,
                unchanged: 0,
                ignored: 2,
                removed: 0,
                prune_authorized: true,
                failed: Vec::new(),
            }
        );
        assert!(!db
            .observe_sacd_image(
                root_id,
                2,
                &root.join("Some DVD/movie.iso").to_string_lossy(),
                0,
                0,
                super::SACD_PARSER_REVISION,
            )
            .unwrap());
    }

    #[test]
    fn raw_sacd_scan_imports_once_and_keeps_tracks_when_the_image_is_truncated() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("music");
        std::fs::create_dir(&root).unwrap();
        let image_path = root.join("physical.ISO");
        let mut logical = vec![0u8; 640 * 2048];
        let master = &mut logical[510 * 2048..511 * 2048];
        master[..8].copy_from_slice(b"SACDMTOC");
        master[8..10].copy_from_slice(&[1, 20]);
        master[0x40..0x44].copy_from_slice(&544u32.to_be_bytes());
        master[0x54..0x56].copy_from_slice(&3u16.to_be_bytes());
        let toc = &mut logical[544 * 2048..547 * 2048];
        toc[..8].copy_from_slice(b"TWOCHTOC");
        toc[8..12].copy_from_slice(&[1, 20, 0, 3]);
        toc[0x14] = 4;
        toc[0x15] = 2;
        toc[0x20] = 2;
        toc[0x42] = 4;
        toc[0x45] = 1;
        toc[0x48..0x4c].copy_from_slice(&600u32.to_be_bytes());
        toc[0x4c..0x50].copy_from_slice(&619u32.to_be_bytes());
        toc[2048..2056].copy_from_slice(b"SACDTRL1");
        toc[2056..2060].copy_from_slice(&600u32.to_be_bytes());
        toc[3076..3080].copy_from_slice(&20u32.to_be_bytes());
        toc[4096..4104].copy_from_slice(b"SACDTRL2");
        toc[5126] = 4;
        let mut file = std::fs::File::create(&image_path).unwrap();
        for sector in logical.chunks_exact(2048) {
            file.write_all(&[0xa5; 12]).unwrap();
            file.write_all(sector).unwrap();
            file.write_all(&[0x5a; 4]).unwrap();
        }
        drop(file);

        // Manual open builds the same rows without requiring catalogue import.
        let rows = super::build_image_rows(&image_path, &SacdLabels::default()).unwrap();
        assert_eq!(rows.album, "physical");
        assert_eq!(rows.tracks.len(), 1);
        assert_eq!(
            rows.tracks[0].file_path,
            format!("sacd:{}#1", image_path.display())
        );
        let db = LibraryDatabase::open(&temp.path().join("library.db")).unwrap();
        let root_id = db
            .add_folder_with_network_info(&root.to_string_lossy(), false, None)
            .unwrap();
        let scan = |generation| {
            scan_root_for_sacd(
                &db,
                root_id,
                generation,
                &root,
                &SacdLabels::default(),
                &AtomicBool::new(false),
            )
        };
        let first = scan(1);
        assert_eq!(first.imported, 1);
        assert!(first.failed.is_empty());
        let tracks = db.get_all_track_paths().unwrap();
        assert_eq!(tracks.len(), 1);
        let second = scan(2);
        assert_eq!(second.unchanged, 1);
        assert_eq!(second.imported, 0);
        assert_eq!(db.get_all_track_paths().unwrap(), tracks);

        // Even a missing physical trailer is a failed SACD read, not a file
        // that may be silently ignored and have its known tracks pruned.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&image_path)
            .unwrap()
            .set_len(640 * 2064 - 4)
            .unwrap();
        let third = scan(3);
        assert_eq!(third.failed.len(), 1);
        assert_eq!(third.ignored, 0);
        assert_eq!(third.removed, 0);
        assert_eq!(db.get_all_track_paths().unwrap(), tracks);
    }

    #[test]
    fn cancel_stops_before_the_first_candidate() {
        let temp = TempDir::new().unwrap();
        let db = LibraryDatabase::open(&temp.path().join("library.db")).unwrap();
        write_plain_iso(&temp.path().join("movie.iso"));
        let root_id = db
            .add_folder_with_network_info(&temp.path().to_string_lossy(), false, None)
            .unwrap();
        let summary = scan_root_for_sacd(
            &db,
            root_id,
            1,
            temp.path(),
            &SacdLabels::default(),
            &AtomicBool::new(true),
        );
        assert_eq!(summary, SacdScanSummary::default());
    }
}
