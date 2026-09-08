//! Decode complete SACD tracks, check their exact DSD64 byte count and print
//! deterministic per-channel hashes without writing the copyrighted audio.
//!
//! `cargo run --release -p qbz-dsd --example sacd_verify -- <image.iso> [track]`

use qbz_dsd::{DsdDemuxer, SacdDemuxer};

const BYTES_PER_CHANNEL_FRAME: u64 = 4_704;
const BITS_PER_CHANNEL_FRAME: u64 = BYTES_PER_CHANNEL_FRAME * 8;
const READ_FRAMES: usize = 64;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let image = std::path::PathBuf::from(
        args.next()
            .ok_or("usage: sacd_verify <image.iso> [track]")?,
    );
    let selected = args.next().map(|value| value.parse::<u8>()).transpose()?;
    if args.next().is_some() {
        return Err("usage: sacd_verify <image.iso> [track]".into());
    }

    let area = qbz_disc::sacd::read_area(&image)?;
    let tracks = area
        .tracks
        .iter()
        .filter(|track| selected.is_none_or(|number| track.number == number))
        .collect::<Vec<_>>();
    if tracks.is_empty() {
        return Err(format!("track {} does not exist", selected.unwrap_or_default()).into());
    }

    for track in tracks {
        let mut demux = SacdDemuxer::open_default(&image, track)?;
        let expected = u64::from(track.duration_frames) * BYTES_PER_CHANNEL_FRAME;
        let midpoint_frame = u64::from(track.duration_frames / 2);
        let suffix_start = midpoint_frame * BYTES_PER_CHANNEL_FRAME;
        let mut total = 0u64;
        let mut hashes = [FNV_OFFSET; 2];
        let mut suffix_hashes = [FNV_OFFSET; 2];
        let mut first_frame_hashes = None;
        let mut last_frame_hashes = [FNV_OFFSET; 2];
        let mut output = vec![Vec::new(), Vec::new()];
        loop {
            output.iter_mut().for_each(Vec::clear);
            let read =
                demux.read_planar(&mut output, READ_FRAMES * BYTES_PER_CHANNEL_FRAME as usize)?;
            if read == 0 {
                break;
            }
            if output.iter().any(|channel| channel.len() != read) {
                return Err(
                    format!("track {} returned unequal channel lengths", track.number).into(),
                );
            }
            total = total
                .checked_add(read as u64)
                .ok_or("decoded byte count overflow")?;
            let suffix_offset = suffix_start.saturating_sub(total - read as u64) as usize;
            for (channel_index, ((hash, suffix_hash), channel)) in hashes
                .iter_mut()
                .zip(&mut suffix_hashes)
                .zip(&output)
                .enumerate()
            {
                *hash = hash_bytes(*hash, channel);
                if suffix_offset < channel.len() {
                    *suffix_hash = hash_bytes(*suffix_hash, &channel[suffix_offset..]);
                }
                last_frame_hashes[channel_index] = hash_bytes(
                    FNV_OFFSET,
                    &channel[channel.len() - BYTES_PER_CHANNEL_FRAME as usize..],
                );
            }
            if first_frame_hashes.is_none() {
                first_frame_hashes = Some([
                    hash_bytes(FNV_OFFSET, &output[0][..BYTES_PER_CHANNEL_FRAME as usize]),
                    hash_bytes(FNV_OFFSET, &output[1][..BYTES_PER_CHANNEL_FRAME as usize]),
                ]);
            }
        }
        if total != expected {
            return Err(format!(
                "track {} decoded {total} bytes/channel, expected {expected}",
                track.number
            )
            .into());
        }
        println!(
            "track {:>3}: {total} bytes/channel fnv1a64={:016x}/{:016x}",
            track.number, hashes[0], hashes[1]
        );

        // A selected-track run also proves that the seek estimate reacquires
        // the exact timecoded frame even when DST compression varies across
        // the track. Compare the suffix to a fresh decoder, which exercises
        // decoder reset as well as container positioning.
        if selected.is_some() {
            let mut seeked = SacdDemuxer::open_default(&image, track)?;
            seeked.seek_to_bit(midpoint_frame * BITS_PER_CHANNEL_FRAME)?;
            let mut seek_total = 0u64;
            let mut seek_hashes = [FNV_OFFSET; 2];
            loop {
                output.iter_mut().for_each(Vec::clear);
                let read = seeked
                    .read_planar(&mut output, READ_FRAMES * BYTES_PER_CHANNEL_FRAME as usize)?;
                if read == 0 {
                    break;
                }
                seek_total += read as u64;
                for (hash, channel) in seek_hashes.iter_mut().zip(&output) {
                    *hash = hash_bytes(*hash, channel);
                }
            }
            if seek_total != expected - suffix_start || seek_hashes != suffix_hashes {
                return Err(format!(
                    "track {} midpoint seek disagrees with its linear suffix: bytes={seek_total}/{} hash={:016x}/{:016x} expected={:016x}/{:016x}",
                    track.number,
                    expected - suffix_start,
                    seek_hashes[0],
                    seek_hashes[1],
                    suffix_hashes[0],
                    suffix_hashes[1],
                )
                .into());
            }
            println!("           midpoint seek verified at frame {midpoint_frame}");

            let last_frame = u64::from(track.duration_frames)
                .checked_sub(1)
                .ok_or("zero-duration track")?;
            seeked.seek_to_bit(last_frame * BITS_PER_CHANNEL_FRAME)?;
            output.iter_mut().for_each(Vec::clear);
            let read = seeked.read_planar(&mut output, BYTES_PER_CHANNEL_FRAME as usize)?;
            let last_seek_hashes = [
                hash_bytes(FNV_OFFSET, &output[0]),
                hash_bytes(FNV_OFFSET, &output[1]),
            ];
            if read as u64 != BYTES_PER_CHANNEL_FRAME || last_seek_hashes != last_frame_hashes {
                return Err(format!("track {} last-frame seek disagrees", track.number).into());
            }
            output.iter_mut().for_each(Vec::clear);
            if seeked.read_planar(&mut output, BYTES_PER_CHANNEL_FRAME as usize)? != 0 {
                return Err(format!("track {} emitted data past EOF", track.number).into());
            }

            seeked.seek_to_bit(expected * 8)?;
            output.iter_mut().for_each(Vec::clear);
            if seeked.read_planar(&mut output, BYTES_PER_CHANNEL_FRAME as usize)? != 0 {
                return Err(format!("track {} EOF seek emitted data", track.number).into());
            }

            seeked.seek_to_bit(0)?;
            output.iter_mut().for_each(Vec::clear);
            let read = seeked.read_planar(&mut output, BYTES_PER_CHANNEL_FRAME as usize)?;
            let start_seek_hashes = [
                hash_bytes(FNV_OFFSET, &output[0]),
                hash_bytes(FNV_OFFSET, &output[1]),
            ];
            if read as u64 != BYTES_PER_CHANNEL_FRAME
                || Some(start_seek_hashes) != first_frame_hashes
            {
                return Err(format!("track {} repeated start seek disagrees", track.number).into());
            }
            println!("           last-frame, EOF and repeated-start seeks verified");
        }
    }
    Ok(())
}
