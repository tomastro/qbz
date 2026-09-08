//! Integration tests over synthesized DSF/DFF files.

use qbz_dsd::{open_dsd, DsdError, DsdPcmConverter};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Minimal valid DSF: `groups` block-groups of 0x69 silence.
fn write_dsf(name: &str, channels: u32, rate: u32, groups: usize, metadata: Option<&[u8]>) -> PathBuf {
    let block_size = 4096u32;
    let bytes_per_ch = groups * block_size as usize;
    let sample_count = (bytes_per_ch as u64) * 8;
    let data_len = (bytes_per_ch * channels as usize) as u64;
    // DSD chunk (28) + fmt chunk (52, header included) + data header (12).
    let file_len_without_meta = 28 + 52 + 12 + data_len;
    let metadata_ptr = if metadata.is_some() { file_len_without_meta } else { 0 };

    let mut f = Vec::new();
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&(file_len_without_meta + metadata.map_or(0, |m| m.len() as u64)).to_le_bytes());
    f.extend_from_slice(&metadata_ptr.to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    f.extend_from_slice(&1u32.to_le_bytes()); // version
    f.extend_from_slice(&0u32.to_le_bytes()); // format id = DSD raw
    f.extend_from_slice(&2u32.to_le_bytes()); // channel type (stereo)
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&1u32.to_le_bytes()); // bits per sample: 1 = LSB first
    f.extend_from_slice(&sample_count.to_le_bytes());
    f.extend_from_slice(&block_size.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes()); // reserved
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(12 + data_len).to_le_bytes());
    for _ in 0..groups {
        for _ in 0..channels {
            f.extend_from_slice(&vec![0x69u8; block_size as usize]);
        }
    }
    if let Some(m) = metadata {
        f.extend_from_slice(m);
    }
    let path = tmp(name);
    std::fs::File::create(&path).unwrap().write_all(&f).unwrap();
    path
}

/// Minimal DFF: FRM8 { FVER, PROP(SND){ FS, CHNL, CMPR }, DSD data }.
fn write_dff(name: &str, channels: u16, rate: u32, data_bytes_total: usize, cmpr: &[u8; 4]) -> PathBuf {
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(b"FS  ");
    prop.extend_from_slice(&4u64.to_be_bytes());
    prop.extend_from_slice(&rate.to_be_bytes());
    prop.extend_from_slice(b"CHNL");
    let chnl_len = 2 + 4 * channels as u64;
    prop.extend_from_slice(&chnl_len.to_be_bytes());
    prop.extend_from_slice(&channels.to_be_bytes());
    for _ in 0..channels {
        prop.extend_from_slice(b"SLFT");
    }
    if chnl_len % 2 == 1 {
        prop.push(0);
    }
    prop.extend_from_slice(b"CMPR");
    // Compression subchunk: 4-byte ID + pascal-ish name (we write just the ID
    // + a 1-byte count 0, padded).
    prop.extend_from_slice(&5u64.to_be_bytes());
    prop.extend_from_slice(cmpr);
    prop.push(0);
    prop.push(0); // even padding

    let mut f = Vec::new();
    f.extend_from_slice(b"FRM8");
    f.extend_from_slice(&0u64.to_be_bytes()); // form size (unused by reader)
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(b"FVER");
    f.extend_from_slice(&4u64.to_be_bytes());
    f.extend_from_slice(&[1, 5, 0, 0]);
    f.extend_from_slice(b"PROP");
    f.extend_from_slice(&(prop.len() as u64).to_be_bytes());
    f.extend_from_slice(&prop);
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(&(data_bytes_total as u64).to_be_bytes());
    f.extend_from_slice(&vec![0x69u8; data_bytes_total]);
    let path = tmp(name);
    std::fs::File::create(&path).unwrap().write_all(&f).unwrap();
    path
}

#[test]
fn dsf_parses_and_converts_to_exact_frame_count() {
    let path = write_dsf("silence64.dsf", 2, 2_822_400, 2, None);
    let demux = open_dsd(&path).unwrap();
    let info = demux.info().clone();
    assert_eq!(info.dsd_rate, 2_822_400);
    assert_eq!(info.channels, 2);
    assert_eq!(info.sample_count, 2 * 4096 * 8);
    assert!(info.lsb_first);

    let mut conv = DsdPcmConverter::new(demux, -6.0).unwrap();
    let expected_frames = info.sample_count / 32; // DSD64 → 88.2k
    assert_eq!(conv.total_frames(), expected_frames);
    let mut frames = 0u64;
    let mut peak = 0f32;
    while let Some(block) = conv.next_block().unwrap() {
        frames += (block.len() / 2) as u64;
        for s in block {
            peak = peak.max(s.abs());
        }
    }
    assert_eq!(frames, expected_frames);
    assert!(peak < 0.01, "silence converted loud: peak {peak}");
}

#[test]
fn dsf_reads_embedded_id3() {
    let mut tag = id3::Tag::new();
    use id3::TagLike;
    tag.set_title("Karma Police");
    tag.set_artist("Radiohead");
    tag.set_album("OK Computer");
    tag.set_track(6);
    let mut blob = Vec::new();
    tag.write_to(&mut blob, id3::Version::Id3v24).unwrap();

    let path = write_dsf("tagged.dsf", 2, 2_822_400, 1, Some(&blob));
    let demux = open_dsd(&path).unwrap();
    let tags = &demux.info().tags;
    assert_eq!(tags.title.as_deref(), Some("Karma Police"));
    assert_eq!(tags.artist.as_deref(), Some("Radiohead"));
    assert_eq!(tags.album.as_deref(), Some("OK Computer"));
    assert_eq!(tags.track_number, Some(6));
}

#[test]
fn dsf_dsd128_uses_three_stages() {
    let path = write_dsf("silence128.dsf", 2, 5_644_800, 2, None);
    let demux = open_dsd(&path).unwrap();
    let mut conv = DsdPcmConverter::new(demux, -6.0).unwrap();
    assert_eq!(conv.total_frames(), (2 * 4096 * 8) / 64);
    let mut frames = 0u64;
    while let Some(block) = conv.next_block().unwrap() {
        frames += (block.len() / 2) as u64;
    }
    assert_eq!(frames, (2 * 4096 * 8) / 64);
}

#[test]
fn dsf_eight_channels_rejected() {
    let path = write_dsf("multi8.dsf", 8, 2_822_400, 1, None);
    match open_dsd(&path) {
        Err(DsdError::UnsupportedChannels(8)) => {}
        Err(other) => panic!("expected UnsupportedChannels, got {other:?}"),
        Ok(_) => panic!("expected UnsupportedChannels, got Ok"),
    }
}

#[test]
fn dsf_5_1_downmixes_to_stereo() {
    let path = write_dsf("multi51.dsf", 6, 2_822_400, 1, None);
    let demux = open_dsd(&path).unwrap();
    assert_eq!(demux.info().channels, 6);
    let mut conv = DsdPcmConverter::new(demux, -6.0).unwrap();
    assert_eq!(conv.channels(), 2);
    let expected_frames = demux_total_frames(4096 * 8);
    assert_eq!(conv.total_frames(), expected_frames);
    let mut frames = 0u64;
    let mut peak = 0f32;
    while let Some(block) = conv.next_block().unwrap() {
        frames += (block.len() / 2) as u64;
        for s in block {
            peak = peak.max(s.abs());
        }
    }
    assert_eq!(frames, expected_frames);
    assert!(peak < 0.01, "5.1 silence downmix not silent: peak {peak}");
}

fn demux_total_frames(sample_count: u64) -> u64 {
    sample_count / 32 // DSD64 → 88.2 kHz
}

#[test]
fn dop_stream_frames_and_markers() {
    use qbz_dsd::DopStream;
    let path = write_dsf("dop64.dsf", 2, 2_822_400, 1, None);
    let demux = open_dsd(&path).unwrap();
    let mut dop = DopStream::new(demux).unwrap();
    assert_eq!(dop.carrier_rate(), 176_400);
    assert_eq!(dop.total_frames(), 4096 * 8 / 16);
    let words: Vec<i32> = dop.by_ref().collect();
    assert_eq!(words.len() as u64, dop.total_frames() * 2);
    // DSF silence is 0x69 LSB-first → bit-reversed payload is 0x96, and the
    // markers alternate per frame across both channels.
    assert_eq!(words[0], ((0x05 << 16) | 0x9696) << 8);
    assert_eq!(words[1], ((0x05 << 16) | 0x9696) << 8);
    assert_eq!(words[2], ((0xFA << 16) | 0x9696) << 8);
}

#[test]
fn dff_parses_stereo() {
    let path = write_dff("plain.dff", 2, 2_822_400, 8192, b"DSD ");
    let demux = open_dsd(&path).unwrap();
    let info = demux.info();
    assert_eq!(info.dsd_rate, 2_822_400);
    assert_eq!(info.channels, 2);
    assert_eq!(info.sample_count, 8192 / 2 * 8);
    assert!(!info.lsb_first);
}

#[test]
fn dsf_seek_handles_channel_blocks_and_an_in_block_offset() {
    const DATA_START: u64 = 28 + 52 + 12;
    const BLOCK: u64 = 4096;
    let path = write_dsf("seek-blocks.dsf", 2, 2_822_400, 2, None);
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    // Second block-group, byte 7 in each channel block.
    file.seek(SeekFrom::Start(DATA_START + 2 * BLOCK + 7))
        .unwrap();
    file.write_all(&[0x12]).unwrap();
    file.seek(SeekFrom::Start(DATA_START + 3 * BLOCK + 7))
        .unwrap();
    file.write_all(&[0x34]).unwrap();

    let mut demux = open_dsd(&path).unwrap();
    demux.seek_to_bit((BLOCK + 7) * 8).unwrap();
    let mut planar = vec![Vec::new(), Vec::new()];
    assert!(demux.read_planar(&mut planar, BLOCK as usize).unwrap() > 0);
    assert_eq!(planar[0][0], 0x12);
    assert_eq!(planar[1][0], 0x34);
}

#[test]
fn dff_seek_preserves_interleaved_channel_alignment() {
    let path = write_dff("seek-interleaved.dff", 2, 2_822_400, 128, b"DSD ");
    let bytes = std::fs::read(&path).unwrap();
    let data_header = bytes
        .windows(4)
        .rposition(|window| window == b"DSD ")
        .unwrap();
    let data_start = data_header as u64 + 12;
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(data_start + 34)).unwrap();
    file.write_all(&[0x56, 0x78]).unwrap();

    let mut demux = open_dsd(&path).unwrap();
    demux.seek_to_bit(17 * 8).unwrap();
    let mut planar = vec![Vec::new(), Vec::new()];
    assert_eq!(demux.read_planar(&mut planar, 4).unwrap(), 4);
    assert_eq!(planar[0][0], 0x56);
    assert_eq!(planar[1][0], 0x78);
}

#[test]
fn dff_dst_rejected() {
    let path = write_dff("dst.dff", 2, 2_822_400, 128, b"DST ");
    match open_dsd(&path) {
        Err(DsdError::UnsupportedDst) => {}
        Err(other) => panic!("expected UnsupportedDst, got {other:?}"),
        Ok(_) => panic!("expected UnsupportedDst, got Ok"),
    }
}

#[test]
fn garbage_rejected() {
    let path = tmp("garbage.bin");
    std::fs::write(&path, b"definitely not dsd").unwrap();
    assert!(matches!(open_dsd(&path), Err(DsdError::Corrupt(_))));
}
