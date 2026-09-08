//! Walk a real ISO without mounting it.
//! `cargo run -p qbz-disc --example iso_probe -- <path.iso>`
fn main() {
    let path = std::env::args().nth(1).expect("usage: iso_probe <file.iso>");
    let p = std::path::PathBuf::from(&path);
    let mut iso = match qbz_disc::iso9660::IsoImage::open(&p) {
        Ok(i) => i,
        Err(e) => { println!("open failed: {e}"); return; }
    };
    println!("root: {:?}", iso.root());
    let root = iso.root().clone();
    for e in iso.read_dir(&root).unwrap_or_default() {
        println!("  /{:<16} lsn {:>8}  {:>12} bytes  {}", e.name, e.lsn, e.size,
                 if e.is_dir { "DIR" } else { "" });
    }
    if let Ok(Some(audio)) = iso.find("/2C_AUDIO") {
        println!("/2C_AUDIO:");
        let kids = iso.read_dir(&audio).unwrap_or_default();
        for e in kids.iter().take(6) {
            println!("  {:<16} lsn {:>8}  {:>12} bytes", e.name, e.lsn, e.size);
        }
        println!("  … {} entries total", kids.len());
    }
    // The structure the SACD reader will actually ask for.
    for want in ["/MASTER1.TOC", "/2C_AUDIO/2C_AREA1.TOC", "/2C_AUDIO/TRACK001.2CH"] {
        match iso.find(want) {
            Ok(Some(e)) => println!("find {want} -> lsn {} size {}", e.lsn, e.size),
            Ok(None) => println!("find {want} -> NOT FOUND"),
            Err(e) => println!("find {want} -> error {e}"),
        }
    }
}
