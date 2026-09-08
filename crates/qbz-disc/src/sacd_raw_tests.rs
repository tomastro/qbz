//! Synthetic logical/physical copies must expose identical TOCs and audio.
use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

const LOGICAL: usize = 2048;
const PHYSICAL: usize = 2064;
const SECTORS: usize = 640; // Also makes the physical file divisible by 2048.

struct TestImage(PathBuf);

impl TestImage {
    fn write(logical: &[u8], physical: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "qbz-sacd-raw-{}-{}.iso",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        if physical {
            for sector in logical.chunks_exact(LOGICAL) {
                file.write_all(&[0xa5; 12]).unwrap();
                file.write_all(sector).unwrap();
                file.write_all(&[0x5a; 4]).unwrap();
            }
        } else {
            file.write_all(logical).unwrap();
        }
        Self(path)
    }
}

impl Drop for TestImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn payload(frame: usize, dst: bool) -> Vec<u8> {
    (0..if dst { 3015 } else { 9408 })
        .map(|i| ((frame * 31 + i) % 251) as u8)
        .collect()
}

fn fixture(dst: bool) -> Vec<u8> {
    let mut data = vec![0; SECTORS * LOGICAL];
    let span = if dst { 2 } else { 5 };
    for lsn in [510, 520, 530] {
        let master = &mut data[lsn * LOGICAL..(lsn + 1) * LOGICAL];
        master[..8].copy_from_slice(b"SACDMTOC");
        master[8..10].copy_from_slice(&[1, 20]);
        master[0x40..0x44].copy_from_slice(&544u32.to_be_bytes());
        master[0x44..0x48].copy_from_slice(&548u32.to_be_bytes());
        master[0x54..0x56].copy_from_slice(&3u16.to_be_bytes());
    }
    let mut toc = vec![0; 3 * LOGICAL];
    toc[..8].copy_from_slice(b"TWOCHTOC");
    toc[8..12].copy_from_slice(&[1, 20, 0, 3]);
    toc[0x14] = 4;
    toc[0x15] = if dst { 0 } else { 2 };
    toc[0x20] = 2;
    toc[0x42] = 4;
    toc[0x45] = 2;
    toc[0x48..0x4c].copy_from_slice(&600u32.to_be_bytes());
    toc[0x4c..0x50].copy_from_slice(&(600u32 + 4 * span as u32 - 1).to_be_bytes());
    toc[LOGICAL..LOGICAL + 8].copy_from_slice(b"SACDTRL1");
    toc[2 * LOGICAL..2 * LOGICAL + 8].copy_from_slice(b"SACDTRL2");
    for track in 0..2 {
        let at = LOGICAL + 8 + track * 4;
        toc[at..at + 4].copy_from_slice(&(600u32 + (track * 2 * span) as u32).to_be_bytes());
        toc[at + 1020..at + 1024].copy_from_slice(&(2 * span as u32).to_be_bytes());
        toc[at + LOGICAL + 2] = (track * 2) as u8;
        toc[at + LOGICAL + 1022] = 2;
    }
    for lsn in [544, 548] {
        data[lsn * LOGICAL..(lsn + 3) * LOGICAL].copy_from_slice(&toc);
    }
    for frame in 0..4 {
        let audio = payload(frame, dst);
        let mut used = 0;
        for index in 0..span {
            let lsn = 600 + frame * span + index;
            let sector = &mut data[lsn * LOGICAL..(lsn + 1) * LOGICAL];
            let first = index == 0;
            if dst {
                sector[0] = if first { 0x25 } else { 0x21 };
                let at = if first { 7 } else { 3 };
                let len = (LOGICAL - at).min(audio.len() - used);
                let descriptor = 0x1000 | if first { 0x8000 } else { 0 } | len as u16;
                sector[1..3].copy_from_slice(&descriptor.to_be_bytes());
                if first {
                    sector[5] = frame as u8;
                    sector[6] = 2 << 2;
                }
                sector[at..at + len].copy_from_slice(&audio[used..used + len]);
                used += len;
            } else {
                sector[0] = if first { 0x44 } else { 0x40 };
                let header = if first { 8 } else { 5 };
                let len = 2016.min(audio.len() - used);
                let padding = LOGICAL - header - len;
                sector[1..3].copy_from_slice(&(0x1800 | padding as u16).to_be_bytes());
                sector[3..5].copy_from_slice(
                    &(0x1000 | if first { 0x8000 } else { 0 } | len as u16).to_be_bytes(),
                );
                if first {
                    sector[7] = frame as u8;
                }
                sector[LOGICAL - len..].copy_from_slice(&audio[used..used + len]);
                used += len;
            }
        }
        assert_eq!(used, audio.len());
    }
    data
}

#[test]
fn raw_and_logical_copies_preserve_geometry_frames_and_seeks() {
    for dst in [false, true] {
        let bytes = fixture(dst);
        let logical = TestImage::write(&bytes, false);
        let raw = TestImage::write(&bytes, true);
        assert_eq!(std::fs::metadata(&raw.0).unwrap().len() % 2048, 0);
        let expected = read_area(&logical.0).unwrap();
        let actual = read_area(&raw.0).unwrap();
        assert_eq!(actual.fingerprint(), expected.fingerprint());
        assert_eq!(actual.tracks, expected.tracks);
        assert_eq!(sniff_sacd_image(&raw.0).unwrap(), SacdSniff::Sacd);
        for (index, track) in actual.tracks.iter().enumerate() {
            let mut reader = SacdFrameReader::open(&raw.0, track).unwrap();
            for frame in index * 2..index * 2 + 2 {
                let read = reader.next_frame().unwrap().unwrap();
                assert_eq!(read.payload, payload(frame, dst));
                assert_eq!(read.timecode_frame, frame as u32);
            }
            assert!(reader.next_frame().unwrap().is_none());
            for offset in [1, 0, 2, 1] {
                reader.seek_to_fraction(offset, 2).unwrap();
                let read = reader.next_frame().unwrap();
                if offset == 2 {
                    assert!(read.is_none());
                } else {
                    assert_eq!(
                        read.unwrap().payload,
                        payload(index * 2 + offset as usize, dst)
                    );
                }
            }
        }
    }
}

#[test]
fn raw_batch_reads_strip_every_header_and_trailer_and_keep_bounds() {
    let bytes = fixture(false);
    let raw = TestImage::write(&bytes, true);
    let mut reader = SectorImage::open(&raw.0).unwrap();
    for (lsn, count) in [(0, 1), (510, 30), (600, 20), (639, 1), (640, 0)] {
        assert_eq!(
            reader.read_sectors(lsn as u64, count).unwrap(),
            bytes[lsn * LOGICAL..(lsn + count) * LOGICAL]
        );
    }
    assert!(reader.read_sectors(640, 1).is_err());
    assert!(reader.read_sectors(u64::MAX, 2).is_err());
    let area = read_area(&raw.0).unwrap();
    let mut legacy = SacdTrackReader::open(&raw.0, &area.tracks[0]).unwrap();
    let mut audio = Vec::new();
    legacy.next_chunk(&mut audio, 200).unwrap();
    assert_eq!(audio, [payload(0, false), payload(1, false)].concat());
}

#[test]
fn raw_layout_can_be_detected_from_only_the_last_master_copy() {
    let mut bytes = fixture(false);
    for lsn in [510, 520] {
        bytes[lsn * LOGICAL..lsn * LOGICAL + 8].fill(0);
    }
    let raw = TestImage::write(&bytes, true);
    assert_eq!(sniff_sacd_image(&raw.0).unwrap(), SacdSniff::Sacd);
    assert_eq!(read_area(&raw.0).unwrap().tracks.len(), 2);
}

#[test]
fn raw_partial_sectors_are_errors_even_when_only_the_trailer_is_missing() {
    for removed in [1, 4, 12, PHYSICAL - 1] {
        let raw = TestImage::write(&fixture(false), true);
        let track = read_area(&raw.0).unwrap().tracks.remove(0);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&raw.0)
            .unwrap()
            .set_len((SECTORS * PHYSICAL - removed) as u64)
            .unwrap();
        assert!(matches!(
            sniff_sacd_image(&raw.0),
            Err(SacdError::InvalidImageLength(_))
        ));
        assert!(matches!(
            read_area(&raw.0),
            Err(SacdError::InvalidImageLength(_))
        ));
        assert!(matches!(
            SacdFrameReader::open(&raw.0, &track),
            Err(SacdError::InvalidImageLength(_))
        ));
    }
    let raw = TestImage::write(&fixture(false), true);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&raw.0)
        .unwrap()
        .set_len((510 * PHYSICAL + 12 + 8) as u64)
        .unwrap();
    assert!(matches!(
        sniff_sacd_image(&raw.0),
        Err(SacdError::InvalidImageLength(_))
    ));
}

#[test]
fn raw_geometry_cannot_point_into_a_missing_whole_sector() {
    let raw = TestImage::write(&fixture(false), true);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&raw.0)
        .unwrap()
        .set_len((610 * PHYSICAL) as u64)
        .unwrap();
    assert!(matches!(read_area(&raw.0), Err(SacdError::MalformedToc(_))));
}

#[test]
fn raw_conflicting_area_copies_are_still_rejected() {
    let mut bytes = fixture(false);
    bytes[550 * LOGICAL + 8 + 1020 + 2] = 3;
    let raw = TestImage::write(&bytes, true);
    assert!(matches!(
        read_area(&raw.0),
        Err(SacdError::ConflictingAreaTocs)
    ));
}

#[test]
fn raw_images_cannot_choose_between_two_signed_layouts() {
    let raw = TestImage::write(&fixture(false), true);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&raw.0)
        .unwrap();
    file.seek(SeekFrom::Start(510 * 2048)).unwrap();
    file.write_all(b"SACDMTOC").unwrap();
    assert!(matches!(
        sniff_sacd_image(&raw.0),
        Err(SacdError::AmbiguousSectorLayout)
    ));
}

#[test]
fn raw_reads_keep_the_changed_file_guard() {
    let raw = TestImage::write(&fixture(false), true);
    let mut reader = SectorImage::open(&raw.0).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&raw.0)
        .unwrap()
        .write_all(&[0])
        .unwrap();
    assert!(matches!(
        reader.read_sectors(0, 350),
        Err(SacdError::ImageChangedDuringRead(_))
    ));
}

#[test]
fn raw_sized_unsigned_files_are_not_sacd_images() {
    let raw = TestImage::write(&vec![0; SECTORS * LOGICAL], true);
    assert_eq!(sniff_sacd_image(&raw.0).unwrap(), SacdSniff::NotSacd);
}
