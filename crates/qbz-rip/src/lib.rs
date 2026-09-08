//! "Rip this disc into my library": CD audio in, tagged FLAC out.
//!
//! Frontend-agnostic (ADR-006). It knows about discs, samples and files; it
//! knows nothing about windows, and it reports progress through a callback so
//! a UI can drive it without this crate depending on one.
//!
//! PORTABILITY, which shaped the design: reading a CD is the only part that
//! differs between platforms. Linux talks to the drive through ioctls; macOS
//! MOUNTS an audio CD and presents each track as an AIFF file under
//! /Volumes, so there the "read" is a file read. [`RipSource`] is that seam,
//! and everything after it — encoding, tagging, naming — is shared.
//!
//! Encoding is `flacenc`, pure Rust: no libFLAC to build, nothing extra to
//! bundle into Flatpak, Snap or AppImage, and macOS gets it for free.

use std::path::{Path, PathBuf};

/// Where one track's audio comes from.
pub enum RipSource {
    /// Sectors off a drive (Linux).
    Cd {
        device: PathBuf,
        start_lsn: u32,
        sectors: u32,
    },
    /// A file that already holds the audio — the macOS mounted-CD case, and
    /// the seam a future "rip this folder" would reuse. The decoding is the
    /// caller's, so this crate stays free of an audio-decoder dependency.
    Samples { pcm: Vec<i32> },
}

/// One track to write.
pub struct RipTrack {
    pub number: u32,
    pub title: String,
    pub artist: String,
    pub source: RipSource,
}

/// A whole job.
pub struct RipPlan {
    /// Directory the ALBUM folder is created inside — the destination the
    /// user picked, never guessed.
    pub destination: PathBuf,
    pub album: String,
    pub album_artist: String,
    pub year: Option<u32>,
    pub tracks: Vec<RipTrack>,
    /// MusicBrainz DiscID, when the disc had one. Goes in the log: it is the
    /// one identity anybody else can recompute from the same disc.
    pub disc_id: Option<String>,
    /// This crate's own TOC hash — the key the app remembers the disc under.
    pub toc_fingerprint: Option<String>,
    /// How many tracks the DISC has, when the plan covers only some of them.
    /// A partial rip that does not say so reads as a disc with four tracks.
    pub disc_track_count: usize,
    /// An image to drop in the album folder as `cover.jpg`. Copied, never
    /// moved — the source is the app's artwork cache and is not ours.
    pub cover: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum RipError {
    #[error("the destination {0} is not a writable directory")]
    Destination(PathBuf),
    #[error("reading the disc failed on track {track}: {source}")]
    Read {
        track: u32,
        #[source]
        source: qbz_disc::CdError,
    },
    #[error("encoding track {track} failed: {why}")]
    Encode { track: u32, why: String },
    #[error("writing {path} failed: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cancelled")]
    Cancelled,
}

/// Progress, reported per track and per chunk within it.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub track_index: usize,
    pub track_count: usize,
    /// 0.0 to 1.0 within the current track.
    pub fraction: f32,
}

/// A filename that survives every filesystem the app runs on.
///
/// The separators are the obvious part. The rest is the part that bites:
/// Windows refuses a handful of characters and a set of reserved DEVICE names,
/// macOS stores `:` as a path separator in some APIs, and a trailing dot or
/// space is silently dropped by Windows — turning "Track 1." into "Track 1"
/// and two tracks into a collision. A ripped library is likely to be copied
/// between machines, so this sanitises for the strictest of them.
pub fn safe_filename(raw: &str) -> String {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    s = s.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() {
        s = "untitled".to_string();
    }
    let stem = s.split('.').next().unwrap_or(&s).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    // Long titles exist (classical especially); leave room for a number
    // prefix and an extension inside the usual 255-byte limit.
    if s.len() > 180 {
        s.truncate(180);
        s = s.trim().to_string();
    }
    s
}

/// Decode raw CD-DA sectors into the interleaved i32 samples the encoder
/// wants.
///
/// The drive hands back little-endian 16-bit stereo — MEASURED, not assumed
/// (`qbz_disc::cdda` documents the 285x margin that settled it). Widening to
/// i32 is what `flacenc` takes; the sample VALUES are untouched, so the FLAC
/// is a bit-exact copy of what came off the disc.
pub fn cdda_to_samples(raw: &[u8], out: &mut Vec<i32>) {
    out.reserve(raw.len() / 2);
    for pair in raw.chunks_exact(2) {
        out.push(i16::from_le_bytes([pair[0], pair[1]]) as i32);
    }
}

/// Encode interleaved 16-bit samples as FLAC bytes.
pub fn encode_flac(samples: &[i32], channels: usize, rate: usize) -> Result<Vec<u8>, String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| format!("encoder config: {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(samples, channels, 16, rate);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| format!("encode: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| format!("serialise: {e:?}"))?;
    let mut bytes = sink.as_slice().to_vec();
    declare_fixed_blocksize(&mut bytes);
    Ok(bytes)
}

/// Make STREAMINFO declare a FIXED block size, which is what the frames say.
///
/// This is not cosmetic — without it the file cannot be SEEKED.
///
/// A stream's last frame is normally short: 12 709 620 samples at a 4096
/// block leaves 3828 for the final one. `flacenc` reports that honestly as
/// `min_blocksize = 3828, max_blocksize = 4096` — but in FLAC, `min != max`
/// MEANS variable-blocksize, and a decoder reading a variable-blocksize
/// stream expects each frame header to carry a SAMPLE number. The frames
/// carry FRAME numbers, correctly, because the stream really is fixed. So
/// libFLAC warns once per frame ("sample or frame number does not increase
/// correctly … file might not be seekable") and every seek fails with
/// `FLAC__STREAM_DECODER_SEEK_ERROR`. Measured on a real rip: 3102 warnings
/// over 3103 frames, and `flac -d --skip=2:00` refused outright.
///
/// libFLAC's own encoder writes `min == max` for exactly this reason; a short
/// final frame is expected and does not make a stream variable. Two bytes.
///
/// Layout: "fLaC" (4) + block header (4) + STREAMINFO, whose first field is
/// min_blocksize (u16 BE) and second is max_blocksize.
fn declare_fixed_blocksize(bytes: &mut [u8]) {
    const MIN_AT: usize = 8;
    const MAX_AT: usize = 10;
    if bytes.len() < MAX_AT + 2 || &bytes[0..4] != b"fLaC" {
        return;
    }
    // Block type 0 = STREAMINFO. If the first block is anything else the file
    // is not shaped the way this assumes, and doing nothing is right.
    if bytes[4] & 0x7F != 0 {
        return;
    }
    let (a, b) = (bytes[MAX_AT], bytes[MAX_AT + 1]);
    bytes[MIN_AT] = a;
    bytes[MIN_AT + 1] = b;
}

/// Write Vorbis comments onto a finished FLAC file.
pub fn tag_flac(
    path: &Path,
    album: &str,
    album_artist: &str,
    artist: &str,
    title: &str,
    track: u32,
    total: u32,
    year: Option<u32>,
) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, ItemKey, TagExt};
    use lofty::probe::Probe;
    use lofty::tag::{Tag, TagType};

    let mut tagged = Probe::open(path)
        .map_err(|e| format!("open for tagging: {e}"))?
        .read()
        .map_err(|e| format!("read for tagging: {e}"))?;
    let tag = match tagged.primary_tag_mut() {
        Some(t) => t,
        None => {
            tagged.insert_tag(Tag::new(TagType::VorbisComments));
            tagged
                .primary_tag_mut()
                .ok_or_else(|| "no tag after insert".to_string())?
        }
    };
    tag.set_title(title.to_string());
    tag.set_artist(artist.to_string());
    tag.set_album(album.to_string());
    tag.set_track(track);
    tag.set_track_total(total);
    if let Some(y) = year {
        // `set_year` is not on lofty's Accessor for every tag type; the
        // Vorbis comment a FLAC actually carries is DATE, so write that.
        tag.insert_text(ItemKey::RecordingDate, y.to_string());
    }
    // ALBUMARTIST is what a library groups a compilation by; without it a
    // disc with per-track artists scatters into one album per track.
    tag.insert_text(ItemKey::AlbumArtist, album_artist.to_string());
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("write tags: {e}"))
}

/// Run a rip.
///
/// `progress` is called often enough to move a bar and is where a UI also
/// decides to STOP: returning `false` cancels, and the partial file of the
/// track in flight is deleted. A half-written FLAC left in a music folder is
/// worse than no file — it scans, it plays, and it ends early.
///
/// Files land in `destination/<album artist> - <album>/NN - <title>.flac`.
/// The folder is created; an existing file is overwritten only after the new
/// one is fully encoded, so a failure never destroys a previous rip.
pub fn rip<F>(plan: &RipPlan, mut progress: F) -> Result<Vec<PathBuf>, RipError>
where
    F: FnMut(Progress) -> bool,
{
    if !plan.destination.is_dir() {
        return Err(RipError::Destination(plan.destination.clone()));
    }
    let folder = plan.destination.join(safe_filename(&format!(
        "{} - {}",
        plan.album_artist, plan.album
    )));
    std::fs::create_dir_all(&folder).map_err(|e| RipError::Write {
        path: folder.clone(),
        source: e,
    })?;

    let total = plan.tracks.len();
    let mut written = Vec::with_capacity(total);
    let mut receipts: Vec<Receipt> = Vec::with_capacity(total);

    for (i, track) in plan.tracks.iter().enumerate() {
        let name = format!(
            "{:02} - {}.flac",
            track.number,
            safe_filename(&track.title)
        );
        let path = folder.join(&name);

        // Read the whole track before encoding. A CD track is at most ~170 MB
        // of PCM and the encoder wants the samples anyway; streaming it would
        // buy nothing but complexity here.
        let mut samples: Vec<i32> = Vec::new();
        match &track.source {
            RipSource::Samples { pcm } => samples.extend_from_slice(pcm),
            RipSource::Cd {
                device,
                start_lsn,
                sectors,
            } => {
                let toc_track = qbz_disc::TocTrack {
                    number: track.number as u8,
                    start_lsn: *start_lsn,
                    sectors: *sectors,
                    is_audio: true,
                };
                let mut reader = qbz_disc::cdda::TrackReader::open(device, &toc_track)
                    .map_err(|e| RipError::Read {
                        track: track.number,
                        source: e,
                    })?;
                let mut raw = Vec::new();
                samples.reserve(*sectors as usize * qbz_disc::CDDA_SECTOR_BYTES / 2);
                loop {
                    let n = reader.next_chunk(&mut raw).map_err(|e| RipError::Read {
                        track: track.number,
                        source: e,
                    })?;
                    if n == 0 {
                        break;
                    }
                    cdda_to_samples(&raw, &mut samples);
                    let done = samples.len() as f32
                        / (*sectors as usize * qbz_disc::CDDA_SECTOR_BYTES / 2).max(1) as f32;
                    // Reading is most of the wall clock, so the bar tracks it
                    // and the encode is the last slice.
                    if !progress(Progress {
                        track_index: i,
                        track_count: total,
                        fraction: (done * 0.9).min(0.9),
                    }) {
                        return Err(RipError::Cancelled);
                    }
                }
            }
        }

        let flac = encode_flac(&samples, 2, 44_100).map_err(|why| RipError::Encode {
            track: track.number,
            why,
        })?;
        // Write to a temporary name and rename: an interrupted write must not
        // leave a truncated .flac behind that looks like a finished track.
        let tmp = path.with_extension("flac.part");
        std::fs::write(&tmp, &flac).map_err(|e| RipError::Write {
            path: tmp.clone(),
            source: e,
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| RipError::Write {
            path: path.clone(),
            source: e,
        })?;
        // Tag AFTER the rename, never before. `lofty` identifies a file by its
        // EXTENSION, and `.flac.part` is not one it knows — tagging the
        // temporary file silently wrote nothing, and the first rip came out
        // with a STREAMINFO block and no Vorbis comments at all.
        if let Err(why) = tag_flac(
            &path,
            &plan.album,
            &plan.album_artist,
            &track.artist,
            &track.title,
            track.number,
            total as u32,
            plan.year,
        ) {
            // A missing tag is a blemish; a missing FILE is a failed rip.
            log::warn!("[rip] track {} tags not written: {why}", track.number);
        }

        log::info!(
            "[rip] {} -> {} ({} samples, {} bytes)",
            track.number,
            path.display(),
            samples.len(),
            flac.len()
        );
        receipts.push(Receipt {
            number: track.number,
            title: track.title.clone(),
            file: name,
            sectors: match &track.source {
                RipSource::Cd { sectors, .. } => Some(*sectors),
                RipSource::Samples { .. } => None,
            },
            frames: samples.len() / 2,
            bytes: flac.len(),
            md5: audio_md5(&flac),
        });
        written.push(path);

        if !progress(Progress {
            track_index: i,
            track_count: total,
            fraction: 1.0,
        }) {
            return Err(RipError::Cancelled);
        }
    }

    // The two things that make a folder self-describing. Neither is allowed to
    // fail the rip: the audio is the deliverable, and a missing sidecar is a
    // blemish on a folder full of correct files.
    if let Some(src) = plan.cover.as_ref() {
        if let Err(e) = copy_cover(src, &folder) {
            log::warn!("[rip] cover not copied: {e}");
        }
    }
    if let Err(e) = std::fs::write(folder.join(LOG_NAME), write_log(plan, &receipts)) {
        log::warn!("[rip] log not written: {e}");
    }

    Ok(written)
}

/// The file name the log lands under, beside the FLACs.
pub const LOG_NAME: &str = "qbz-rip.log";

/// What one finished track is on record as being.
struct Receipt {
    number: u32,
    title: String,
    file: String,
    /// `None` for a file source (the macOS mounted-CD path), where "sectors"
    /// is not a fact about anything.
    sectors: Option<u32>,
    /// Stereo frames, i.e. samples per channel.
    frames: usize,
    bytes: usize,
    md5: Option<String>,
}

/// The MD5 of the DECODED audio, read back out of the file's own STREAMINFO.
///
/// `flacenc` computes it while encoding and writes it there, so this is not a
/// second checksum invented for the log — it is the one already inside every
/// FLAC, which is why `flac -t` and `metaflac --show-md5sum` can check it
/// without QBZ being involved at all.
///
/// STREAMINFO is 34 bytes and the digest is its last 16: "fLaC" (4) + block
/// header (4) + 18 bytes of stream fields, then the digest. An all-zero digest
/// means the encoder declined to compute one and is reported as absent rather
/// than as a checksum of zeros.
fn audio_md5(flac: &[u8]) -> Option<String> {
    const AT: usize = 8 + 18;
    if flac.len() < AT + 16 || &flac[0..4] != b"fLaC" {
        return None;
    }
    let digest = &flac[AT..AT + 16];
    if digest.iter().all(|b| *b == 0) {
        return None;
    }
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Put the artwork in the folder under the name every scanner already looks
/// for. The EXTENSION follows the source, because renaming a PNG to .jpg is
/// how a picture stops opening.
fn copy_cover(src: &Path, folder: &Path) -> std::io::Result<()> {
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "jpg" | "jpeg" | "png" | "webp"))
        .unwrap_or_else(|| "jpg".to_string());
    let ext = if ext == "jpeg" { "jpg".to_string() } else { ext };
    std::fs::copy(src, folder.join(format!("cover.{ext}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filename_survives_every_filesystem_it_might_land_on() {
        assert_eq!(safe_filename("AC/DC: Back in Black?"), "AC_DC_ Back in Black_");
        // Windows drops a trailing dot silently, which would collide two
        // tracks whose titles differ only by it.
        assert_eq!(safe_filename("Intro."), "Intro");
        assert_eq!(safe_filename("   "), "untitled");
        // A reserved DEVICE name is not a filename on Windows even with an
        // extension: CON.flac fails.
        assert_eq!(safe_filename("CON"), "_CON");
        assert_eq!(safe_filename("con.flac"), "_con.flac");
        assert_eq!(safe_filename("Concerto"), "Concerto");
    }

    #[test]
    fn a_long_title_is_trimmed_with_room_for_the_rest_of_the_name() {
        let long = "a".repeat(300);
        let out = safe_filename(&long);
        assert!(out.len() <= 180);
        // "07 - " plus ".flac" still has to fit a 255-byte name.
        assert!(out.len() + 5 + 5 < 255);
    }

    #[test]
    fn cd_bytes_widen_to_samples_without_changing_a_value() {
        // Little-endian, measured on the owner's drive. If this ever reads as
        // big-endian the FLAC is noise, so the values are pinned.
        let raw = [0x00, 0x80, 0xFF, 0x7F, 0x01, 0x00];
        let mut out = Vec::new();
        cdda_to_samples(&raw, &mut out);
        assert_eq!(out, vec![i16::MIN as i32, i16::MAX as i32, 1]);
    }

    #[test]
    fn streaminfo_declares_a_fixed_block_size_so_the_file_can_be_seeked() {
        // Deliberately NOT a multiple of the block size, which is the normal
        // case and the one that broke: the last frame is short, flacenc
        // reports min != max, and every decoder then treats the stream as
        // variable-blocksize and refuses to seek.
        let samples = vec![0i32; 44_100 * 2 + 1234];
        let flac = encode_flac(&samples, 2, 44_100).expect("encode");
        let min = u16::from_be_bytes([flac[8], flac[9]]);
        let max = u16::from_be_bytes([flac[10], flac[11]]);
        assert_eq!(min, max, "min != max makes the stream variable-blocksize");
        assert!(max > 0);
    }

    #[test]
    fn the_patch_leaves_anything_that_is_not_a_flac_alone() {
        let mut junk = *b"NOTAFLACnnnnnnnn";
        let before = junk;
        declare_fixed_blocksize(&mut junk);
        assert_eq!(junk, before);
    }

    #[test]
    fn a_second_of_silence_round_trips_through_the_encoder() {
        // Not a quality test — a wiring test. It proves the encoder is
        // reachable, produces a real FLAC stream, and agrees about the shape
        // of the samples it is handed.
        let samples = vec![0i32; 44_100 * 2];
        let flac = encode_flac(&samples, 2, 44_100).expect("encode a second of silence");
        assert_eq!(&flac[0..4], b"fLaC", "must start with the FLAC magic");
        // Silence compresses hard; anything near the PCM size means the
        // encoder did not actually run.
        assert!(
            flac.len() < samples.len() * 2 / 10,
            "silence compressed to {} bytes",
            flac.len()
        );
    }

    #[test]
    fn a_plan_with_samples_writes_tagged_files_in_a_named_folder() {
        // End to end without a drive: two short tracks straight to disk.
        let dir = std::env::temp_dir().join(format!("qbz-rip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = RipPlan {
            destination: dir.clone(),
            album: "Fear Inoculum".into(),
            album_artist: "Tool".into(),
            year: Some(2019),
            tracks: vec![
                RipTrack {
                    number: 1,
                    title: "Fear Inoculum".into(),
                    artist: "Tool".into(),
                    source: RipSource::Samples { pcm: vec![0i32; 8820] },
                },
                RipTrack {
                    number: 7,
                    title: "7empest".into(),
                    artist: "Tool".into(),
                    source: RipSource::Samples { pcm: vec![0i32; 8820] },
                },
            ],
            disc_id: Some("BeNBMsD8Du5NO2W61Yk.B2jwwIs-".into()),
            toc_fingerprint: Some("38bef21351f7fca3".into()),
            disc_track_count: 7,
            cover: None,
        };
        let out = rip(&plan, |_| true).expect("rip");
        assert_eq!(out.len(), 2);
        assert!(out[0].ends_with("Tool - Fear Inoculum/01 - Fear Inoculum.flac"));
        assert!(out[1].ends_with("Tool - Fear Inoculum/07 - 7empest.flac"));
        for p in &out {
            let b = std::fs::read(p).unwrap();
            assert_eq!(&b[0..4], b"fLaC");
        }
        // The tags must actually BE there. The first version of this tagged
        // the `.part` file, lofty ignored it for want of a known extension,
        // and every ripped track came out with no Vorbis comments — a failure
        // that only shows up if something looks.
        let raw = std::fs::read(&out[0]).unwrap();
        let head = String::from_utf8_lossy(&raw[..raw.len().min(8192)]);
        assert!(head.contains("Fear Inoculum"), "ALBUM tag missing from the file");
        assert!(head.contains("Tool"), "ARTIST tag missing from the file");

        // No .part files may survive a successful rip.
        let leftovers: Vec<_> = std::fs::read_dir(out[0].parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "a partial file survived");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The log is a promise about what it does NOT do. If those three lines
    /// ever drift out, the file starts reading like an EAC log, which is the
    /// one thing it must never be mistaken for.
    #[test]
    fn the_log_says_what_it_is_not_before_it_says_anything_else() {
        let dir = std::env::temp_dir().join(format!("qbz-rip-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = RipPlan {
            destination: dir.clone(),
            album: "Fear Inoculum".into(),
            album_artist: "Tool".into(),
            year: Some(2019),
            tracks: vec![RipTrack {
                number: 2,
                title: "Pneuma".into(),
                artist: "Tool".into(),
                source: RipSource::Samples { pcm: vec![0i32; 8820] },
            }],
            disc_id: Some("BeNBMsD8Du5NO2W61Yk.B2jwwIs-".into()),
            toc_fingerprint: Some("38bef21351f7fca3".into()),
            // ONE track written out of SEVEN on the disc — a partial rip.
            disc_track_count: 7,
            cover: None,
        };
        rip(&plan, |_| true).expect("rip");

        let log = std::fs::read_to_string(dir.join("Tool - Fear Inoculum").join(LOG_NAME))
            .expect("the log is written beside the files");
        for disclaimer in ["test-and-copy", "read offset", "AccurateRip"] {
            assert!(
                log.contains(disclaimer),
                "the log must say it does not do {disclaimer}"
            );
        }
        // The identity anybody else can recompute.
        assert!(log.contains("BeNBMsD8Du5NO2W61Yk.B2jwwIs-"));
        assert!(log.contains("38bef21351f7fca3"));
        // A partial rip has to SAY it is partial, or one track reads as the
        // whole album.
        assert!(log.contains("1 of 7"), "partial rips are named as such:\n{log}");
        assert!(log.contains("Pneuma"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The digest in the log has to be the one INSIDE the file, or `flac -t`
    /// and this log disagree and the log is the thing people would trust.
    #[test]
    fn the_logged_md5_is_the_files_own_streaminfo_digest() {
        let flac = encode_flac(&vec![0i32; 8820], 2, 44_100).expect("encode");
        let logged = audio_md5(&flac).expect("a digest");
        assert_eq!(logged.len(), 32, "32 hex characters");
        // Byte for byte the STREAMINFO tail: "fLaC" + 4-byte block header +
        // 18 bytes of fields, then the 16-byte digest.
        let from_file: String = flac[26..42].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(logged, from_file);
    }

    #[test]
    fn a_file_that_is_not_a_flac_yields_no_digest() {
        assert!(audio_md5(b"not a flac at all, really").is_none());
        // An all-zero digest means the encoder declined to compute one; that
        // is ABSENT, not "the checksum of an empty file".
        let mut fake = vec![0u8; 42];
        fake[0..4].copy_from_slice(b"fLaC");
        assert!(audio_md5(&fake).is_none());
    }

    /// A picture renamed to the wrong extension is a picture that stops
    /// opening, so the copy follows the source.
    #[test]
    fn the_cover_keeps_its_own_format_and_lands_beside_the_tracks() {
        let dir = std::env::temp_dir().join(format!("qbz-rip-cover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("art.png");
        std::fs::write(&src, b"\x89PNG\r\n\x1a\n").unwrap();
        let plan = RipPlan {
            destination: dir.clone(),
            album: "A".into(),
            album_artist: "B".into(),
            year: None,
            tracks: vec![RipTrack {
                number: 1,
                title: "T".into(),
                artist: "B".into(),
                source: RipSource::Samples { pcm: vec![0i32; 8820] },
            }],
            disc_id: None,
            toc_fingerprint: None,
            disc_track_count: 1,
            cover: Some(src),
        };
        rip(&plan, |_| true).expect("rip");
        let folder = dir.join("B - A");
        assert!(folder.join("cover.png").is_file(), "a PNG stays a PNG");
        assert!(!folder.join("cover.jpg").exists());
        // A cover that cannot be copied must not fail the rip.
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The date arithmetic is the only place in this file that could be
    /// quietly wrong for years. Two known dates, one of them a leap day.
    #[test]
    fn the_calendar_conversion_is_right_on_the_days_that_break_it() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2000 is a leap year, 1900 was not — the two the naive rule gets
        // backwards.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn cancelling_stops_and_leaves_no_partial_track() {
        let dir = std::env::temp_dir().join(format!("qbz-rip-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = RipPlan {
            destination: dir.clone(),
            album: "A".into(),
            album_artist: "B".into(),
            year: None,
            tracks: vec![RipTrack {
                number: 1,
                title: "T".into(),
                artist: "B".into(),
                source: RipSource::Samples { pcm: vec![0i32; 8820] },
            }],
            disc_id: None,
            toc_fingerprint: None,
            disc_track_count: 1,
            cover: None,
        };
        let err = rip(&plan, |_| false).unwrap_err();
        assert!(matches!(err, RipError::Cancelled));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_destination_that_is_not_a_directory_is_refused_up_front() {
        let plan = RipPlan {
            destination: PathBuf::from("/definitely/not/here"),
            album: "A".into(),
            album_artist: "B".into(),
            year: None,
            tracks: vec![],
            disc_id: None,
            toc_fingerprint: None,
            disc_track_count: 0,
            cover: None,
        };
        assert!(matches!(rip(&plan, |_| true), Err(RipError::Destination(_))));
    }
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

/// A PROVENANCE record, deliberately not an EAC log.
///
/// An EAC or XLD log is trusted for what it PROVES, not for how it looks:
/// test-and-copy (rip twice, compare), a drive read-offset correction, and an
/// AccurateRip cross-check against thousands of other people's rips of the
/// same disc. QBZ does none of the three. A file that imitated that format
/// without the method would be worse than no file at all — it invites a claim
/// this ripper cannot back, and the people who read rip logs are exactly the
/// people who would check.
///
/// So the header says what this is and is not, in the first block, not in
/// small print. What is left is genuinely useful and genuinely true:
///
///   * WHICH DISC — the MusicBrainz DiscID and the TOC fingerprint, both
///     recomputable by anyone holding the same disc.
///   * WHICH DRIVE — straight from sysfs.
///   * PER TRACK — the audio MD5 that already lives in the file's STREAMINFO,
///     verifiable with `flac -t` without QBZ. It checks the files against
///     THEMSELVES (bit rot, a bad copy), not against the world.
///   * READ ERRORS — zero, and it means zero: an unreadable sector aborts the
///     rip rather than being written as silence. Which is a stronger promise
///     than a burst-mode EAC rip makes.
///
/// English, because it is a technical file that gets shared.
fn write_log(plan: &RipPlan, receipts: &[Receipt]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(2048);

    let _ = writeln!(s, "QBZ rip log");
    let _ = writeln!(s, "===========");
    let _ = writeln!(s);
    let _ = writeln!(s, "WHAT THIS IS, AND WHAT IT IS NOT");
    let _ = writeln!(s, "  A record of where these files came from, not a verification log.");
    let _ = writeln!(s, "  QBZ does NOT do test-and-copy, does NOT apply a drive read offset,");
    let _ = writeln!(s, "  and does NOT check AccurateRip. The checksums below verify these");
    let _ = writeln!(s, "  files against themselves; they will not match an EAC or XLD log,");
    let _ = writeln!(s, "  and are not meant to.");
    let _ = writeln!(s);
    let _ = writeln!(s, "  What it does promise: a sector that cannot be read ABORTS the rip.");
    let _ = writeln!(s, "  Silence is never written in place of audio, so \"0 read errors\"");
    let _ = writeln!(s, "  below means the disc was read in full or you would have no files.");
    let _ = writeln!(s);

    let year = plan.year.map(|y| format!(" ({y})")).unwrap_or_default();
    let _ = writeln!(s, "Ripped        {}", now_utc());
    let _ = writeln!(s, "QBZ           {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(s, "Album         {} - {}{}", plan.album_artist, plan.album, year);

    let drive = plan.tracks.iter().find_map(|t| match &t.source {
        RipSource::Cd { device, .. } => Some(device.clone()),
        RipSource::Samples { .. } => None,
    });
    match drive.as_ref().map(|d| (d, qbz_disc::drive_model(d))) {
        Some((dev, Some(model))) => {
            let _ = writeln!(s, "Drive         {}  ({model})", dev.display());
        }
        // A "device" with no model behind it is the macOS shape: the OS mounted
        // the disc and these bytes came out of ITS reader, not ours. Printing
        // "Drive /Volumes/Fear Inoculum (unknown model)" would name a volume as
        // if it were hardware, which is the log's first chance to lie.
        Some((dev, None)) => {
            let _ = writeln!(s, "Source        {}", dev.display());
            let _ = writeln!(s, "              read by the operating system, not by QBZ");
        }
        None => {
            let _ = writeln!(s, "Source        files");
        }
    }
    let _ = writeln!(s, "Read offset   none applied");
    let _ = writeln!(s, "Encoder       flacenc 0.5, default settings, fixed block size");
    let _ = writeln!(s, "Read errors   0");
    let _ = writeln!(s);

    let _ = writeln!(s, "Disc");
    let _ = writeln!(
        s,
        "  MusicBrainz DiscID   {}",
        plan.disc_id.as_deref().unwrap_or("not computed")
    );
    let _ = writeln!(
        s,
        "  TOC fingerprint      {}",
        plan.toc_fingerprint.as_deref().unwrap_or("not computed")
    );
    // A partial rip that does not say so reads as a four-track album.
    if plan.disc_track_count > receipts.len() {
        let _ = writeln!(
            s,
            "  Tracks               {} of {} on the disc (partial rip)",
            receipts.len(),
            plan.disc_track_count
        );
    } else {
        let _ = writeln!(s, "  Tracks               {}", receipts.len());
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "Tracks");
    for r in receipts {
        let _ = writeln!(s, "  {:>2}  {}", r.number, r.title);
        let _ = writeln!(s, "      file      {}", r.file);
        let secs = r.frames as f64 / 44_100.0;
        let _ = writeln!(
            s,
            "      length    {}:{:05.2}   frames {}   flac {} bytes",
            (secs / 60.0) as u64,
            secs % 60.0,
            r.frames,
            r.bytes
        );
        if let Some(n) = r.sectors {
            let _ = writeln!(s, "      sectors   {n}");
        }
        match &r.md5 {
            Some(md5) => {
                let _ = writeln!(s, "      audio md5 {md5}");
            }
            None => {
                let _ = writeln!(s, "      audio md5 not recorded by the encoder");
            }
        }
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "Verify these files without QBZ:");
    let _ = writeln!(s, "  flac -t *.flac                 (checks each file against its own md5)");
    let _ = writeln!(s, "  metaflac --show-md5sum *.flac  (prints the digests listed above)");
    s
}

/// `2026-08-21 13:20:11 UTC`, with no dependency on a calendar crate.
///
/// UTC and not local time on purpose: a log is read on a different machine, in
/// a different place, years later, and a bare local timestamp with no offset is
/// the kind of fact that quietly means nothing.
fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days` — the standard days-since-epoch to
/// calendar conversion, correct across leap years and centuries. Written out
/// rather than pulled in, because one timestamp is not worth a dependency and
/// this algorithm has not changed since 2013.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
