//! Read a SACD image's stereo area and pull real DSD out of a track.
//! `cargo run -p qbz-disc --example sacd_probe -- <disc.iso>`
fn main() {
    let path = std::env::args().nth(1).expect("usage: sacd_probe <disc.iso>");
    let p = std::path::PathBuf::from(&path);
    let area = match qbz_disc::sacd::read_area(&p) {
        Ok(a) => a,
        Err(e) => { println!("area failed: {e}"); return; }
    };
    println!("album: {:?} | artist: {:?}", area.album, area.artist);
    println!("channels {} | area LSN {}..{} | total {:.2}s ({}:{:02})",
        area.channels, area.track_start_lsn, area.track_end_lsn,
        area.total_playtime_secs,
        area.total_playtime_secs as u32 / 60, area.total_playtime_secs as u32 % 60);
    println!("{} tracks", area.tracks.len());
    for t in area.tracks.iter().take(6) {
        println!("  {:2}  lsn {:>8} len {:>7}  {:>3}:{:02}  {}",
            t.number, t.start_lsn, t.length_lsn,
            t.duration_secs as u32 / 60, t.duration_secs as u32 % 60,
            t.title.as_deref().unwrap_or("—"));
    }
    // Pull audio out of the FIRST track and see whether it looks like DSD.
    let t = &area.tracks[0];
    let mut r = match qbz_disc::sacd::SacdTrackReader::open(&p, t) {
        Ok(r) => r, Err(e) => { println!("reader failed: {e}"); return; }
    };
    let mut buf = Vec::new();
    let mut total = 0usize;
    let mut silence = 0usize;
    // ~2.8 seconds of audio: enough to see whether it is silence or music.
    for _ in 0..10 {
        match r.next_chunk(&mut buf, 100) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                silence += buf.iter().filter(|b| **b == 0x99).count();
            }
            Err(e) => { println!("read failed: {e}"); return; }
        }
    }
    println!("\nextracted {total} bytes of audio payload from track 1");
    println!("0x99 (DSD silence) bytes: {silence} ({}%)", 100 * silence / total.max(1));
    let frames = total as f64 / qbz_disc::sacd::DSD64_STEREO_FRAME as f64;
    println!("that is {frames:.3} DSD64 stereo frames ({:.3}s of audio)", frames / 75.0);
    println!("first 32 bytes: {}", buf.iter().take(32)
        .map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
}
