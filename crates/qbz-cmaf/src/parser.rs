use crate::error::CmafError;

const QBZ_INIT_UUID: [u8; 16] = [
    0xc7, 0xc7, 0x5d, 0xf0, 0xfd, 0xd9, 0x51, 0xe9,
    0x8f, 0xc2, 0x29, 0x71, 0xe4, 0xac, 0xf8, 0xd2,
];
const QBZ_SEGMENT_UUID: [u8; 16] = [
    0x3b, 0x42, 0x12, 0x92, 0x56, 0xf3, 0x5f, 0x75,
    0x92, 0x36, 0x63, 0xb6, 0x9a, 0x1f, 0x52, 0xb2,
];
const FLAC_MAGIC: &[u8; 4] = b"fLaC";

/// Info about one segment from the init segment's segment table.
#[derive(Debug, Clone)]
pub struct SegmentTableEntry {
    /// Byte size of this segment's decrypted FLAC frame data.
    pub byte_len: u32,
    /// Number of audio samples in this segment.
    pub sample_count: u32,
}

/// FLAC header and segment table extracted from the init segment.
pub struct InitInfo {
    pub flac_header: Vec<u8>,
    /// Per-segment sizes (indices 0..n_segments-1 correspond to segments 1..n_segments).
    pub segment_table: Vec<SegmentTableEntry>,
}

/// One frame entry from the segment's QBZ_SEGMENT_UUID box.
pub struct FrameEntry {
    pub size: u32,
    pub flags: u16,
    pub iv: [u8; 8],
}

/// Parsed crypto info from a segment's QBZ_SEGMENT_UUID box.
pub struct SegmentCrypto {
    /// Offset to the start of audio frame data (usually mdat payload).
    pub data_offset: usize,
    /// End of the mdat box content. Data between the last frame entry and this
    /// offset is unencrypted trailing audio that must be included in output.
    pub mdat_end: usize,
    pub entries: Vec<FrameEntry>,
}

/// Walk ISO BMFF boxes and find the first UUID box matching `target_uuid`.
/// Returns `(payload_start, box_end)` where payload_start is after the 16-byte UUID.
fn find_uuid_box(data: &[u8], target_uuid: &[u8; 16]) -> Option<(usize, usize)> {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = read_box_size(data, pos);
        if size < 8 || pos + size > data.len() {
            break;
        }
        if &data[pos + 4..pos + 8] == b"uuid" && pos + 24 <= data.len() {
            if &data[pos + 8..pos + 24] == target_uuid.as_ref() {
                // payload_start = box_start + 8 (header) + 16 (uuid) = box_start + 24
                return Some((pos + 24, pos + size));
            }
        }
        pos += size;
    }
    None
}

/// Walk ISO BMFF boxes and find the `mdat` box.
/// Returns `(data_start, box_end)` where data_start = box_start + 8.
#[allow(dead_code)]
fn find_mdat_box(data: &[u8]) -> Option<(usize, usize)> {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = read_box_size(data, pos);
        if size < 8 || pos + size > data.len() {
            break;
        }
        if &data[pos + 4..pos + 8] == b"mdat" {
            return Some((pos + 8, pos + size));
        }
        pos += size;
    }
    None
}

/// Parse the init segment (segment 0) to extract the FLAC header and segment table.
pub fn parse_init_segment(data: &[u8]) -> Result<InitInfo, CmafError> {
    let (payload_start, box_end) = find_uuid_box(data, &QBZ_INIT_UUID)
        .ok_or_else(|| CmafError::ParseError("init segment: QBZ_INIT_UUID box not found".into()))?;

    let payload = &data[payload_start..box_end];
    parse_init_uuid_payload(payload)
}

/// Parse an audio segment to extract per-frame crypto info.
pub fn parse_segment_crypto(data: &[u8]) -> Result<SegmentCrypto, CmafError> {
    // Walk all top-level boxes to find both UUID and mdat boxes.
    let mut uuid_box_start: Option<usize> = None;
    let mut mdat_end = data.len();

    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = read_box_size(data, pos);
        if size < 8 || pos + size > data.len() {
            break;
        }
        let box_type = &data[pos + 4..pos + 8];
        if box_type == b"uuid" && pos + 24 <= data.len() {
            if &data[pos + 8..pos + 24] == QBZ_SEGMENT_UUID.as_ref() {
                uuid_box_start = Some(pos);
            }
        } else if box_type == b"mdat" {
            mdat_end = pos + size;
        }
        pos += size;
    }

    let box_start = uuid_box_start
        .ok_or_else(|| CmafError::ParseError("audio segment: QBZ_SEGMENT_UUID box not found".into()))?;

    parse_segment_uuid_payload(data, box_start, mdat_end)
}

// --- Internal helpers ---

fn parse_init_uuid_payload(payload: &[u8]) -> Result<InitInfo, CmafError> {
    // Payload layout:
    //   [4B padding/version]
    //   [4B track_id]
    //   [4B file_id]
    //   [4B sample_rate]
    //   [1B bits_per_sample]
    //   [1B channels + 2B padding]
    //   [6B total_samples_count]
    //   [2B raw_data_len]
    //   [raw_data_len bytes: contains FLAC header]
    //   [1B key_id_len]
    //   [key_id_len bytes: key_id]
    //   [2B segment_count]
    //   Per segment: [4B byte_len][4B sample_count]

    if payload.len() < 28 {
        return Err(CmafError::ParseError("init UUID payload too short".into()));
    }

    let mut a = 4; // skip version/padding
    a += 4; // track_id
    a += 4; // file_id
    a += 4; // sample_rate
    a += 1; // bits_per_sample
    a += 3; // channels + padding
    a += 6; // total_samples_count

    if a + 2 > payload.len() {
        return Err(CmafError::ParseError("init UUID payload truncated at raw_len".into()));
    }
    let raw_len = u16::from_be_bytes([payload[a], payload[a + 1]]) as usize;
    a += 2;

    if a + raw_len > payload.len() {
        return Err(CmafError::ParseError(format!(
            "init UUID payload truncated: need {raw_len} raw bytes, have {}",
            payload.len().saturating_sub(a)
        )));
    }
    let raw_data = &payload[a..a + raw_len];
    a += raw_len;

    let flac_pos = raw_data
        .windows(4)
        .position(|w| w == FLAC_MAGIC)
        .ok_or_else(|| CmafError::ParseError("init UUID payload: fLaC magic not found".into()))?;

    // fLaC (4) + STREAMINFO block header (4) + STREAMINFO data (34) = 42 bytes
    let header_len = 4 + 4 + 34;
    if flac_pos + header_len > raw_data.len() {
        return Err(CmafError::ParseError("init UUID payload: STREAMINFO truncated".into()));
    }

    let mut flac_header = raw_data[flac_pos..flac_pos + header_len].to_vec();
    // Set last-metadata-block flag in block header byte
    flac_header[4] |= 0x80;

    if a + 1 > payload.len() {
        return Ok(InitInfo {
            flac_header,
            segment_table: Vec::new(),
        });
    }
    let key_id_len = payload[a] as usize;
    a += 1 + key_id_len;

    let mut segment_table = Vec::new();
    if a + 2 <= payload.len() {
        let seg_count = u16::from_be_bytes([payload[a], payload[a + 1]]) as usize;
        a += 2;

        for _ in 0..seg_count {
            if a + 8 > payload.len() {
                break;
            }
            let byte_len =
                u32::from_be_bytes([payload[a], payload[a + 1], payload[a + 2], payload[a + 3]]);
            a += 4;
            let sample_count =
                u32::from_be_bytes([payload[a], payload[a + 1], payload[a + 2], payload[a + 3]]);
            a += 4;
            segment_table.push(SegmentTableEntry { byte_len, sample_count });
        }
    }

    log::debug!(
        "Init UUID: {} segments in table, FLAC header {} bytes",
        segment_table.len(),
        flac_header.len()
    );

    Ok(InitInfo { flac_header, segment_table })
}

fn parse_segment_uuid_payload(
    data: &[u8],
    uuid_box_start: usize,
    mdat_end: usize,
) -> Result<SegmentCrypto, CmafError> {
    // Layout after box header (8) + UUID (16) = offset 24 from uuid_box_start:
    //   [4B version/padding]
    //   [4B data_offset_raw]   — offset from uuid_box_start to audio data
    //   [1B iv_size]
    //   [3B frame_count (24-bit BE)]
    //   Per frame: [4B size][2B skip][2B flags][iv_size bytes IV]

    let base = uuid_box_start + 24; // start of payload after UUID
    if base + 12 > data.len() {
        return Err(CmafError::ParseError(
            "segment UUID payload too short for header".into(),
        ));
    }

    let mut a = base + 4; // skip 4-byte version/padding

    let data_offset_raw = u32::from_be_bytes([data[a], data[a + 1], data[a + 2], data[a + 3]]);
    let data_offset = uuid_box_start + data_offset_raw as usize;
    a += 4;

    let iv_size = data[a] as usize;
    a += 1;

    let frame_count =
        ((data[a] as usize) << 16) | ((data[a + 1] as usize) << 8) | (data[a + 2] as usize);
    a += 3;

    let entry_size = 4 + 2 + 2 + iv_size; // size + skip + flags + iv
    if a + frame_count * entry_size > data.len() {
        return Err(CmafError::ParseError(format!(
            "segment UUID: not enough data for {frame_count} entries of {entry_size} bytes"
        )));
    }

    let mut entries = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        let size = u32::from_be_bytes([data[a], data[a + 1], data[a + 2], data[a + 3]]);
        a += 4;
        a += 2; // skip 2 unknown bytes
        let flags = u16::from_be_bytes([data[a], data[a + 1]]);
        a += 2;

        let mut iv = [0u8; 8];
        let copy_len = iv_size.min(8);
        iv[..copy_len].copy_from_slice(&data[a..a + copy_len]);
        a += iv_size;

        entries.push(FrameEntry { size, flags, iv });
    }

    Ok(SegmentCrypto { data_offset, mdat_end, entries })
}

fn read_box_size(data: &[u8], pos: usize) -> usize {
    if pos + 8 > data.len() {
        return 0;
    }
    let s = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    match s {
        0 => data.len() - pos,
        1..=7 => 0,
        s => s as usize,
    }
}

#[cfg(test)]
mod parse_truncation_tests {
    use super::*;

    #[test]
    fn truncated_raw_len_is_error() {
        // Build a minimal payload that claims more raw bytes than available:
        // raw_len sits at bytes 26..28, raw data starts at 28, so a 30-byte
        // payload with raw_len=100 must fail.
        let mut p = vec![0u8; 30];
        // set raw_len = 100 at bytes 26..28
        p[26] = 0;
        p[27] = 100;
        match parse_init_uuid_payload(&p) {
            Ok(_) => panic!("expected truncation error"),
            Err(e) => {
                let msg = format!("{e}");
                assert!(msg.contains("truncated") || msg.contains("raw"), "{msg}");
            }
        }
    }
}
