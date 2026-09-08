//! Which channel layout is right — re-measured on ALIGNED frames.
//!
//! The first answer to this question (`qbz-dsd/src/sacd_source.rs`) was taken
//! on track 4, whose first frame begins 1344 bytes into the payload, so it was
//! measuring bits that were already scrambled by a framing bug. This asks
//! again, with the frame sync applied, and it asks a question that scrambling
//! cannot fake.
//!
//! THE TEST. A DSD bit stream is a 1-bit sigma-delta code: the LOCAL MEAN of
//! the bits *is* the waveform. Average 64 consecutive bits and you have a
//! crude 44.1 kHz sample; music makes that sequence swing widely and slowly,
//! while a scrambled stream is a fair coin whose 64-bit mean sits at 0.5 with
//! the variance of a binomial and nothing else. So:
//!
//!   correct layout  -> large std-dev, large peak excursion from 0.5
//!   scrambled       -> std-dev near the binomial floor (~0.5/sqrt(64) = 0.0625)
//!
//! No filter, no FFT, no threshold anybody had to choose: the binomial floor
//! is arithmetic, and the answer is a ratio against it.
//!
//! Run: cargo run -p qbz-disc --example sacd_layout2 -- <image.iso> [track]

use qbz_disc::sacd;

const FRAME: usize = 9408;
const HALF: usize = FRAME / 2;
/// Bits averaged per output sample — DSD64 / 64 = 44.1 kHz.
const DECIM: usize = 64;
/// The std-dev a fair coin produces at this decimation. Anything at this level
/// is noise BY ARITHMETIC, not by opinion.
const COIN_FLOOR: f64 = 0.5 / 8.0; // 0.5 / sqrt(64)

fn bits(byte: u8) -> [f64; 8] {
    let mut out = [0.0; 8];
    for (i, slot) in out.iter_mut().enumerate() {
        // MSB first — SACD, like DFF.
        *slot = ((byte >> (7 - i)) & 1) as f64;
    }
    out
}

/// Decimate a channel's bytes to a waveform and describe its swing.
fn describe(chan: &[u8]) -> (f64, f64) {
    let mut samples = Vec::with_capacity(chan.len() * 8 / DECIM);
    let mut acc = 0.0;
    let mut n = 0usize;
    for b in chan {
        for v in bits(*b) {
            acc += v;
            n += 1;
            if n == DECIM {
                samples.push(acc / DECIM as f64);
                acc = 0.0;
                n = 0;
            }
        }
    }
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let var = samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    let peak = samples
        .iter()
        .map(|s| (s - mean).abs())
        .fold(0.0f64, f64::max);
    (var.sqrt(), peak)
}

fn split_block(payload: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut l = Vec::new();
    let mut r = Vec::new();
    for f in payload.chunks_exact(FRAME) {
        l.extend_from_slice(&f[..HALF]);
        r.extend_from_slice(&f[HALF..]);
    }
    (l, r)
}

fn split_interleaved(payload: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut l = Vec::new();
    let mut r = Vec::new();
    for pair in payload.chunks_exact(2) {
        l.push(pair[0]);
        r.push(pair[1]);
    }
    (l, r)
}

fn report(name: &str, payload: &[u8]) {
    for (how, (l, r)) in [
        ("block-per-channel", split_block(payload)),
        ("byte-interleaved", split_interleaved(payload)),
    ] {
        let (ls, lp) = describe(&l);
        let (rs, rp) = describe(&r);
        println!(
            "  {name:<11} {how:<18} L sd {:.4} ({:>5.1}x coin) peak {:.3} | \
             R sd {:.4} ({:>5.1}x coin) peak {:.3}",
            ls,
            ls / COIN_FLOOR,
            lp,
            rs,
            rs / COIN_FLOOR,
            rp,
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: sacd_layout2 <image.iso> [track]");
        std::process::exit(2);
    };
    let only: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    let path = std::path::PathBuf::from(path);

    let area = sacd::read_area(&path).expect("read_area");
    let Some(track) = area.tracks.iter().find(|t| t.number == only) else {
        eprintln!("no track {only}");
        std::process::exit(1);
    };

    let mut iso = qbz_disc::iso9660::IsoImage::open(&path).expect("open");
    let start = track.start_lsn as u64;
    let want = (track.length_lsn as usize).min(600);
    let raw = iso.read_sectors(start, want).expect("read");

    // Collect the audio payload, and note where the first frame begins.
    let mut payload: Vec<u8> = Vec::new();
    let mut first_start: Option<usize> = None;
    for s in 0..want {
        let lsn = start + s as u64;
        let sector = &raw[s * 2048..(s + 1) * 2048];
        for p in sacd::parse_sector(sector, lsn).expect("parse") {
            if p.data_type != sacd::DATA_TYPE_AUDIO {
                continue;
            }
            if p.frame_start && first_start.is_none() {
                first_start = Some(payload.len());
            }
            payload.extend_from_slice(&sector[p.at..p.at + p.len]);
        }
    }
    let skip = first_start.unwrap_or(0);
    println!(
        "track {} — {} B payload, first frame at {skip}, coin floor sd {:.4}",
        track.number,
        payload.len(),
        COIN_FLOOR
    );

    println!("UNSYNCED (what ships today):");
    report("unsynced", &payload);
    println!("SYNCED (drop {skip} B):");
    report("synced", &payload[skip..]);
}
