//! Decode a SACD track with the REAL converter under each candidate layout,
//! with and without frame sync, and dump interleaved f32 for analysis.
//!
//! This exists because the previous two instruments could not falsify
//! anything: a "where does the energy land" ratio and a "how much does the
//! 64-bit local mean swing" statistic both answered the same for scrambled and
//! clean bits. Music and noise are told apart by the SHAPE of the audible
//! spectrum, so this dumps real PCM and lets a spectrum analyser decide.
//!
//! Run: cargo run --release -p qbz-dsd --example sacd_decode_dump -- \
//!          <image.iso> <track> <seconds> <out-dir>

use qbz_dsd::{ChannelLayout, DsdPcmConverter, SacdDemuxer};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 4 {
        eprintln!("usage: sacd_decode_dump <image.iso> <track> <seconds> <out-dir>");
        std::process::exit(2);
    }
    let path = std::path::PathBuf::from(&a[0]);
    let track_no: u8 = a[1].parse().expect("track");
    let secs: f64 = a[2].parse().expect("seconds");
    let out_dir = std::path::PathBuf::from(&a[3]);
    std::fs::create_dir_all(&out_dir).expect("out dir");

    let area = qbz_disc::sacd::read_area(&path).expect("read_area");
    let track = area
        .tracks
        .iter()
        .find(|t| t.number == track_no)
        .expect("track");

    for layout in [ChannelLayout::BlockPerChannel, ChannelLayout::ByteInterleaved] {
        for sync in [false, true] {
            let demux = SacdDemuxer::open_with(&path, track, layout, sync).expect("open");
            let mut conv = DsdPcmConverter::new(Box::new(demux), 0.0).expect("converter");
            let rate = conv.output_rate();
            let ch = conv.channels() as usize;
            let want = (rate as f64 * secs) as usize * ch;
            let mut pcm: Vec<f32> = Vec::with_capacity(want);
            while pcm.len() < want {
                match conv.next_block() {
                    Ok(Some(block)) => pcm.extend_from_slice(&block),
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("decode failed: {e}");
                        break;
                    }
                }
            }
            pcm.truncate(want);
            let name = format!(
                "t{track_no}-{}-{}.f32",
                match layout {
                    ChannelLayout::BlockPerChannel => "block",
                    ChannelLayout::ByteInterleaved => "interleaved",
                },
                if sync { "synced" } else { "unsynced" }
            );
            let file = out_dir.join(&name);
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            std::fs::write(&file, &bytes).expect("write");
            println!("{name}: {} samples @ {rate} Hz, {ch} ch", pcm.len() / ch);
        }
    }
}
