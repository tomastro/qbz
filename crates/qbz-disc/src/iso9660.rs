//! A read-only ISO 9660 directory walk, without mounting anything.
//!
//! Mounting needs root (or a helper the app has no business shipping), and the
//! two things QBZ wants out of a disc image — a SACD's audio area and the
//! ordinary audio files inside a data image — both only need to find files and
//! read byte ranges. That is a few hundred lines, and it costs no privileges.
//!
//! Scope is deliberately small: the Primary Volume Descriptor, directory
//! records, and extents. No Joliet, no Rock Ridge, no El Torito. What it will
//! NOT do is guess: a file recorded in several extents, or interleaved, is
//! DETECTED and reported rather than flattened to its first extent, because a
//! flattened audio file plays the wrong bytes and nothing upstream can tell.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// ISO 9660 logical sector. The standard permits others; every disc image in
/// practice, and both of the owner's SACDs, use 2048.
pub const SECTOR: u64 = 2048;
/// The volume descriptors start here.
const PVD_LSN: u64 = 16;

#[derive(Debug, thiserror::Error)]
pub enum IsoError {
    #[error("cannot open {0}: {1}")]
    Open(PathBuf, std::io::Error),
    #[error("read failed at sector {lsn}: {source}")]
    Read {
        lsn: u64,
        #[source]
        source: std::io::Error,
    },
    #[error("not an ISO 9660 image (no CD001 descriptor)")]
    NotIso,
    #[error("the image declares a {0}-byte logical block; only 2048 is supported")]
    BlockSize(u16),
    #[error("{0} is recorded in several extents or interleaved — refusing to read it as one range")]
    MultiExtent(String),
    #[error("directory tree is deeper or larger than expected (possible loop)")]
    Malformed,
}

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// As recorded, with the `;1` version suffix already stripped.
    pub name: String,
    pub lsn: u32,
    pub size: u32,
    pub is_dir: bool,
    /// File-flags bit 7: "this is not the final directory record for this
    /// file". A file with it set continues in the NEXT record — its bytes are
    /// several extents, not one range.
    ///
    /// This is not theoretical. `2C_TAREA.2CH` on the owner's Rheingold Disc 1
    /// is recorded as FOUR records: three of 1 073 739 776 bytes and one of
    /// 128 579 584. A reader that takes the first and calls it the file reads
    /// 1 GiB of a 3.35 GB area and reports success.
    pub multi_extent: bool,
}

impl Entry {
    pub fn offset(&self) -> u64 {
        self.lsn as u64 * SECTOR
    }
}

pub struct IsoImage {
    file: File,
    path: PathBuf,
    root: Entry,
}

impl IsoImage {
    pub fn open(path: &Path) -> Result<Self, IsoError> {
        let mut file =
            File::open(path).map_err(|e| IsoError::Open(path.to_path_buf(), e))?;
        let pvd = read_sector(&mut file, PVD_LSN)?;

        // Descriptor type 1 = primary, then the "CD001" standard identifier.
        if &pvd[1..6] != b"CD001" {
            return Err(IsoError::NotIso);
        }
        let block = u16::from_le_bytes([pvd[128], pvd[129]]);
        if block as u64 != SECTOR {
            return Err(IsoError::BlockSize(block));
        }

        // The root directory RECORD is embedded in the PVD at offset 156, in
        // the same 34-byte shape as any other directory record.
        let root = parse_record(&pvd[156..190], 0)
            .ok_or(IsoError::Malformed)?
            .0;

        Ok(Self {
            file,
            path: path.to_path_buf(),
            root: Entry {
                name: "/".to_string(),
                ..root
            },
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn root(&self) -> &Entry {
        &self.root
    }

    /// Entries of a directory, excluding the `.` and `..` records.
    pub fn read_dir(&mut self, dir: &Entry) -> Result<Vec<Entry>, IsoError> {
        if !dir.is_dir {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let sectors = dir.size.div_ceil(SECTOR as u32) as u64;
        // A directory that claims to be enormous is corrupt, not interesting.
        if sectors > 4096 {
            return Err(IsoError::Malformed);
        }
        for s in 0..sectors {
            let buf = read_sector(&mut self.file, dir.lsn as u64 + s)?;
            let mut at = 0usize;
            while at < buf.len() {
                let len = buf[at] as usize;
                // A zero length means "no more records in THIS sector" — the
                // next one continues the directory. It is not the end.
                if len == 0 {
                    break;
                }
                if at + len > buf.len() {
                    return Err(IsoError::Malformed);
                }
                if let Some((entry, special)) = parse_record(&buf[at..at + len], len) {
                    if !special {
                        out.push(entry);
                    }
                }
                at += len;
            }
        }
        Ok(out)
    }

    /// Resolve an absolute path like `/2C_AUDIO/TRACK001.2CH`.
    ///
    /// Matching ignores the `;1` version suffix and is case-insensitive: ISO
    /// 9660 level 1 upper-cases everything, and a caller should not have to
    /// know that.
    pub fn find(&mut self, path: &str) -> Result<Option<Entry>, IsoError> {
        let mut current = self.root.clone();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let entries = self.read_dir(&current)?;
            match entries
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(part))
            {
                // A multi-extent file is REFUSED here rather than returned
                // truncated. Returning the first extent would hand back a
                // plausible `Entry` whose bytes are a quarter of the file, and
                // nothing downstream could tell.
                Some(e) if e.multi_extent => {
                    return Err(IsoError::MultiExtent(e.name));
                }
                Some(e) => current = e,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// Read a whole file's extent into memory.
    ///
    /// `cap` bounds the allocation: the size comes from the IMAGE, which is
    /// untrusted input, and a corrupt record must not be able to ask for a
    /// gigabyte. Callers that want a huge file should stream it instead.
    pub fn read_file(&mut self, entry: &Entry, cap: usize) -> Result<Vec<u8>, IsoError> {
        if entry.is_dir {
            return Ok(Vec::new());
        }
        let want = entry.size as usize;
        if want > cap {
            return Err(IsoError::Malformed);
        }
        let mut out = vec![0u8; want];
        self.file
            .seek(SeekFrom::Start(entry.offset()))
            .and_then(|_| self.file.read_exact(&mut out))
            .map_err(|e| IsoError::Read {
                lsn: entry.lsn as u64,
                source: e,
            })?;
        Ok(out)
    }

    /// Read `count` sectors starting at an absolute LSN — the SACD reader's
    /// way in, since it works in sectors rather than files.
    pub fn read_sectors(&mut self, lsn: u64, count: usize) -> Result<Vec<u8>, IsoError> {
        let mut out = vec![0u8; count * SECTOR as usize];
        self.file
            .seek(SeekFrom::Start(lsn * SECTOR))
            .and_then(|_| self.file.read_exact(&mut out))
            .map_err(|e| IsoError::Read { lsn, source: e })?;
        Ok(out)
    }
}

fn read_sector(file: &mut File, lsn: u64) -> Result<[u8; SECTOR as usize], IsoError> {
    let mut buf = [0u8; SECTOR as usize];
    file.seek(SeekFrom::Start(lsn * SECTOR))
        .and_then(|_| file.read_exact(&mut buf))
        .map_err(|e| IsoError::Read { lsn, source: e })?;
    Ok(buf)
}

/// Parse one directory record. Returns the entry and whether it is one of the
/// `.` / `..` self-records, which callers skip.
///
/// Layout (ECMA-119 §9.1), the fields this reader needs:
///   0  length of record
///   2  extent LBA, both-endian (little half at 2, big half at 6)
///   10 data length, both-endian
///   25 file flags
///   32 file identifier length
///   33 file identifier
fn parse_record(rec: &[u8], _len: usize) -> Option<(Entry, bool)> {
    if rec.len() < 34 {
        return None;
    }
    let lsn = u32::from_le_bytes([rec[2], rec[3], rec[4], rec[5]]);
    let size = u32::from_le_bytes([rec[10], rec[11], rec[12], rec[13]]);
    let flags = rec[25];
    let id_len = rec[32] as usize;
    if 33 + id_len > rec.len() {
        return None;
    }
    let raw = &rec[33..33 + id_len];

    // The two self-records are a single 0x00 / 0x01 byte, not text.
    let special = id_len == 1 && (raw[0] == 0 || raw[0] == 1);
    let name = if special {
        if raw[0] == 0 { ".".into() } else { "..".into() }
    } else {
        let s = String::from_utf8_lossy(raw).to_string();
        // ";1" is the version suffix ISO 9660 appends to every FILE name.
        match s.split_once(';') {
            Some((base, _)) => base.to_string(),
            None => s,
        }
    };

    Some((
        Entry {
            name,
            lsn,
            size,
            is_dir: flags & 0x02 != 0,
            multi_extent: flags & 0x80 != 0,
        },
        special,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, lsn: u32, size: u32, flags: u8) -> Vec<u8> {
        let id = name.as_bytes();
        let mut r = vec![0u8; 33 + id.len()];
        r[0] = r.len() as u8;
        r[2..6].copy_from_slice(&lsn.to_le_bytes());
        r[6..10].copy_from_slice(&lsn.to_be_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[14..18].copy_from_slice(&size.to_be_bytes());
        r[25] = flags;
        r[32] = id.len() as u8;
        r[33..].copy_from_slice(id);
        r
    }

    #[test]
    fn a_file_record_decodes_to_its_extent() {
        // TRACK001.2CH on the owner's Disc 2: LSN 1253, 167933952 bytes.
        let r = record("TRACK001.2CH;1", 1253, 167_933_952, 0);
        let (e, special) = parse_record(&r, r.len()).expect("a well-formed record must parse");
        assert!(!special);
        assert_eq!(e.name, "TRACK001.2CH");
        assert_eq!(e.lsn, 1253);
        assert_eq!(e.size, 167_933_952);
        assert!(!e.is_dir);
        // The byte offset a reader seeks to.
        assert_eq!(e.offset(), 1253 * 2048);
    }

    #[test]
    fn the_version_suffix_is_stripped_but_a_dot_in_the_name_survives() {
        let r = record("2C_AREA1.TOC;1", 544, 18_432, 0);
        let (e, _) = parse_record(&r, r.len()).unwrap();
        assert_eq!(e.name, "2C_AREA1.TOC");
    }

    #[test]
    fn the_self_records_are_reported_as_special() {
        for byte in [0u8, 1u8] {
            let mut r = vec![0u8; 34];
            r[0] = 34;
            r[25] = 0x02;
            r[32] = 1;
            r[33] = byte;
            let (_, special) = parse_record(&r, 34).unwrap();
            assert!(special, "the . / .. records must never be listed");
        }
    }

    #[test]
    fn a_directory_is_told_apart_by_its_flag() {
        let d = record("2C_AUDIO", 1_431_814, 2048, 0x02);
        let (e, _) = parse_record(&d, d.len()).unwrap();
        assert!(e.is_dir);
    }

    #[test]
    fn a_multi_extent_file_is_flagged_and_not_passed_off_as_whole() {
        // The real shape of 2C_TAREA.2CH on the owner's Disc 1: the first
        // three records carry the continuation bit, the last does not.
        let cont = record("2C_TAREA.2CH;1", 553, 1_073_739_776, 0x80);
        let (e, _) = parse_record(&cont, cont.len()).unwrap();
        assert!(e.multi_extent, "the continuation bit must be visible");

        let last = record("2C_TAREA.2CH;1", 1_573_414, 128_579_584, 0x00);
        let (e, _) = parse_record(&last, last.len()).unwrap();
        assert!(!e.multi_extent);

        // And an ordinary track file is single-extent, which is why the
        // feature works at all: TRACK001.2CH, Disc 1.
        let track = record("TRACK001.2CH;1", 1253, 197_914_624, 0x00);
        let (e, _) = parse_record(&track, track.len()).unwrap();
        assert!(!e.multi_extent);
    }

    #[test]
    fn a_truncated_record_is_rejected_rather_than_read_past() {
        // The size field of a record is untrusted input from the image.
        assert!(parse_record(&[0u8; 10], 10).is_none());
        let mut r = record("X", 1, 1, 0);
        r[32] = 200; // claims a 200-byte name it does not have
        assert!(parse_record(&r, r.len()).is_none());
    }
}
