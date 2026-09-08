//! Read the disc in the drive and prove the crate agrees with reality.
//! `cargo run -p qbz-disc --example cd_probe`
fn main() {
    let devs = qbz_disc::list_devices();
    println!("devices: {devs:?}");
    let Some(dev) = devs.first() else {
        println!("no drive");
        return;
    };
    let toc = match qbz_disc::read_toc(dev) {
        Ok(t) => t,
        Err(e) => {
            println!("toc failed: {e}");
            return;
        }
    };
    println!("fingerprint: {}", toc.fingerprint());
    println!("leadout LSN: {}", toc.leadout_lsn);
    let mut total = 0u64;
    for t in &toc.tracks {
        let d = t.duration_secs();
        total += d;
        println!(
            "  track {:2}  lsn {:>7}  {:>6} sectors  {:>2}:{:02}  {}",
            t.number,
            t.start_lsn,
            t.sectors,
            d / 60,
            d % 60,
            if t.is_audio { "audio" } else { "DATA (skipped)" }
        );
    }
    println!("total {}:{:02}", total / 60, total % 60);

    // Read the first audio track's opening chunk and prove it is music.
    let Some(first) = toc.audio_tracks().next() else {
        return;
    };
    let mut r = match qbz_disc::cdda::TrackReader::open(dev, first) {
        Ok(r) => r,
        Err(e) => {
            println!("open failed: {e}");
            return;
        }
    };
    let mut buf = Vec::new();
    // Skip the first second — track 1 often opens on silence.
    for _ in 0..2 {
        if let Err(e) = r.next_chunk(&mut buf) {
            println!("read failed: {e}");
            return;
        }
    }
    let samples: Vec<i16> = buf
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let nz = samples.iter().filter(|s| **s != 0).count();
    println!(
        "read {} bytes | {} samples | non-zero {}% | peak {} | byte-swapped? {}",
        buf.len(),
        samples.len(),
        100 * nz / samples.len().max(1),
        peak,
        qbz_disc::cdda::looks_byte_swapped(&buf)
    );
}
