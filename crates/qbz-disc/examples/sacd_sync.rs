//! Where do the DSD frames actually START inside a track's audio payload?
//!
//! `SacdTrackReader::next_chunk` concatenates every audio packet and the
//! demuxer then chops the result into 9408-byte frames, assuming byte 0 of the
//! first packet is byte 0 of a frame. The sector descriptors carry a
//! `frame_start` bit that nobody reads. This measures whether ignoring it is
//! safe: it prints, per track, the offset of the first frame start and whether
//! every later frame start sits on a 9408 multiple from there.
//!
//! Run: cargo run -p qbz-disc --example sacd_sync -- <image.iso> [track]

use qbz_disc::sacd;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: sacd_sync <image.iso> [track]");
        std::process::exit(2);
    };
    let only: Option<u8> = args.next().and_then(|s| s.parse().ok());
    let path = std::path::PathBuf::from(path);

    let area = match sacd::read_area(&path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("read_area failed: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "{} — {} tracks, fingerprint {}",
        path.display(),
        area.tracks.len(),
        area.fingerprint()
    );

    const FRAME: usize = 9408;
    for t in area.tracks.iter().filter(|t| only.is_none_or(|n| t.number == n)) {
        // Walk the track's sectors by hand so the frame_start bits stay
        // visible — `next_chunk` throws them away.
        let mut iso = match qbz_disc::iso9660::IsoImage::open(&path) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("open failed: {e}");
                return;
            }
        };
        let start = t.start_lsn as u64;
        let end = start + t.length_lsn as u64;
        // 400 sectors is over a second of audio — plenty to see the pattern.
        let want = ((end - start) as usize).min(400);
        let raw = match iso.read_sectors(start, want) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("track {}: read failed: {e}", t.number);
                continue;
            }
        };

        let mut audio_len = 0usize;
        let mut starts: Vec<usize> = Vec::new();
        for s in 0..want {
            let lsn = start + s as u64;
            let sector = &raw[s * 2048..(s + 1) * 2048];
            let packets = match sacd::parse_sector(sector, lsn) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("track {} lsn {lsn}: {e}", t.number);
                    break;
                }
            };
            for p in packets {
                if p.data_type != sacd::DATA_TYPE_AUDIO {
                    continue;
                }
                if p.frame_start {
                    starts.push(audio_len);
                }
                audio_len += p.len;
            }
        }

        let first = starts.first().copied();
        // Are the later frame starts exactly one frame apart from the first?
        let aligned_to_first = first.map(|f| {
            starts
                .iter()
                .all(|s| (s - f) % FRAME == 0)
        });
        // And is the NAIVE assumption (frames start at payload byte 0) right?
        let aligned_to_zero = starts.iter().all(|s| s % FRAME == 0);

        println!(
            "track {:>2}: audio {:>8} B in {want} sectors · frame starts {:>3} · \
             first at {:?} · spaced-from-first {:?} · naive-zero-aligned {}",
            t.number,
            audio_len,
            starts.len(),
            first,
            aligned_to_first,
            aligned_to_zero,
        );
        if starts.len() >= 3 {
            let gaps: Vec<usize> = starts.windows(2).map(|w| w[1] - w[0]).collect();
            let odd: Vec<&usize> = gaps.iter().filter(|g| **g != FRAME).collect();
            println!(
                "          gaps: first three {:?} · gaps that are NOT {FRAME}: {}",
                &gaps[..gaps.len().min(3)],
                odd.len()
            );
        }
    }
}
