//! Settle the SACD channel layout with the REAL converter, and leave a WAV of
//! each hypothesis for a human to judge.
//! `cargo run -p qbz-dsd --example sacd_layout -- <disc.iso> <track#> <outdir>`
fn main() {
    let mut a = std::env::args().skip(1);
    let iso = std::path::PathBuf::from(a.next().expect("iso"));
    let tno: u8 = a.next().expect("track").parse().unwrap();
    let outdir = std::path::PathBuf::from(a.next().unwrap_or_else(|| ".".into()));

    let area = qbz_disc::sacd::read_area(&iso).expect("area");
    let track = area.tracks.iter().find(|t| t.number == tno).expect("track");
    println!("track {}: {} ({:.1}s)", track.number,
             track.title.as_deref().unwrap_or("—"), track.duration_secs);

    for (name, layout) in [
        ("A_block", qbz_dsd::ChannelLayout::BlockPerChannel),
        ("B_interleaved", qbz_dsd::ChannelLayout::ByteInterleaved),
    ] {
        let demux = qbz_dsd::SacdDemuxer::open(&iso, track, layout).expect("demux");
        let mut conv = qbz_dsd::DsdPcmConverter::new(Box::new(demux), qbz_dsd::DEFAULT_GAIN_DB)
            .expect("converter");
        let ch = conv.channels();
        let rate = conv.output_rate();
        // ~12 seconds is plenty to hear an orchestra or to hear noise.
        let want = (rate as u64 * 12) as usize;
        let mut pcm: Vec<f32> = Vec::new();
        while pcm.len() < want * ch as usize {
            match conv.next_block() {
                Ok(Some(f)) => pcm.extend_from_slice(&f),
                _ => break,
            }
        }
        let frames = (pcm.len() / ch as usize) as u64;
        // What a human would call "is there music here".
        let peak = pcm.iter().fold(0f32, |m, s| m.max(s.abs()));
        let rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
        // Adjacent-sample difference vs signal energy: real audio at 88.2 kHz
        // moves slowly between samples; scrambled bits do not.
        let d: f32 = pcm.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let e: f32 = pcm.iter().map(|s| s * s).sum();
        println!("{name:16} {frames} frames  peak {peak:.4}  rms {rms:.5}  roughness {:.4}",
                 if e > 0.0 { d / e } else { 0.0 });

        let path = outdir.join(format!("sacd_{name}.wav"));
        let mut out = qbz_dsd::wav_header(frames, ch, rate);
        qbz_dsd::frames_to_pcm24(&pcm, &mut out);
        std::fs::write(&path, out).expect("write");
        println!("                 -> {}", path.display());
    }
}
