//! Rip ONE track off the disc in the drive, tagged, into a directory.
//! `cargo run -p qbz-rip --example rip_one -- <track#> <destdir>`
fn main() {
    let mut a = std::env::args().skip(1);
    let want: u32 = a.next().expect("track number").parse().unwrap();
    let dest = std::path::PathBuf::from(a.next().expect("destination"));

    let dev = qbz_disc::list_devices().into_iter().next().expect("a drive");
    let toc = qbz_disc::read_toc(&dev).expect("toc");
    let starts: Vec<u32> = toc.audio_tracks().map(|t| t.start_lsn).collect();
    let id = qbz_disc::discid::disc_id(&starts, toc.leadout_lsn).expect("disc id");
    println!("disc id {id}");

    let t = toc
        .audio_tracks()
        .find(|t| t.number as u32 == want)
        .expect("that track");
    println!("track {} — {} sectors, {}s", t.number, t.sectors, t.duration_secs());

    let plan = qbz_rip::RipPlan {
        destination: dest,
        album: "Fear Inoculum".into(),
        album_artist: "Tool".into(),
        year: Some(2019),
        tracks: vec![qbz_rip::RipTrack {
            number: t.number as u32,
            title: "Chocolate Chip Trip".into(),
            artist: "Tool".into(),
            source: qbz_rip::RipSource::Cd {
                device: dev.clone(),
                start_lsn: t.start_lsn,
                sectors: t.sectors,
            },
        }],
        disc_id: Some(id.clone()),
        toc_fingerprint: Some(toc.fingerprint()),
        disc_track_count: toc.audio_tracks().count(),
        cover: None,
    };
    let started = std::time::Instant::now();
    let mut last = -1i32;
    let out = qbz_rip::rip(&plan, |p| {
        let pct = (p.fraction * 100.0) as i32;
        if pct / 10 != last / 10 {
            println!("  {pct}%");
            last = pct;
        }
        true
    })
    .expect("rip");
    println!("wrote {:?} in {:.1}s", out, started.elapsed().as_secs_f32());
    for p in &out {
        let md = std::fs::metadata(p).unwrap();
        println!("  {} bytes", md.len());
    }
}
