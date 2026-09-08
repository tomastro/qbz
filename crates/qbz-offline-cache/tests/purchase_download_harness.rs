//! The Purchases download harness (contract §12, owner-approved 2026-08-16).
//!
//! # Why this file exists
//!
//! Nobody on this team can smoke-test Purchases. Qobuz does not sell it in the
//! owner's region, so their account returns `{total: 0, items: []}` forever and
//! the populated screens — and every line of the download pipeline — will first
//! run on a stranger's machine. The contract's §12 established that the Tauri
//! build shipped no harness either: its "mock backend at :8787" turned out to be
//! unserved `fetch` stubs behind `import.meta.env.DEV`, with no server, no
//! fixtures and no npm script. So there is nothing to port, and this is new work.
//! It is also the only thing that turns the feature from ship-blind into
//! ship-testable.
//!
//! # What it covers, and what it does NOT
//!
//! It exercises the DOWNLOAD half for real: a live local HTTP server stands in
//! for the Qobuz CDN, real bytes travel over a real socket through the real
//! client, and the result lands in a real temp directory and a real SQLite
//! database. Path building, the `.part`→rename dance, the registry write, tag
//! writing, cover files and goodies are genuinely executed rather than inspected.
//!
//! It does NOT cover the LIST half. `endpoints::BASE_URL` is a `const`, so
//! `getUserPurchases` cannot be pointed at a local server without changing
//! production code — and that is not worth doing, because the request shape is
//! already pinned by the signature and URL tests in `qbz-qobuz`, and the response
//! shapes by the deserializer tests in `qbz-models` against payloads captured
//! from a live account. Said plainly so nobody reads this file as proof of more
//! than it proves.
//!
//! The server is hand-rolled on `std::net::TcpListener` — about forty lines —
//! rather than pulling a mock-HTTP crate in for one test.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use qbz_library::LibraryDatabase;
use qbz_models::{Goody, Track};
use qbz_offline_cache::purchases_service as svc;

// ─── The fixture server ───────────────────────────────────────────────────────

/// A tiny HTTP/1.1 server serving a fixed route table. Also records the paths it
/// was actually asked for, so a test can assert what was REQUESTED and not
/// merely what came back.
struct FixtureServer {
    base: String,
    hits: mpsc::Receiver<String>,
}

impl FixtureServer {
    fn start(routes: HashMap<String, (u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let port = listener.local_addr().unwrap().port();
        let (tx, hits) = mpsc::channel();

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let routes = routes.clone();
                let tx = tx.clone();
                thread::spawn(move || serve_one(stream, &routes, &tx));
            }
        });

        FixtureServer {
            base: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn requested_paths(&self) -> Vec<String> {
        self.hits.try_iter().collect()
    }
}

fn serve_one(
    mut stream: TcpStream,
    routes: &HashMap<String, (u16, Vec<u8>)>,
    tx: &mpsc::Sender<String>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain the headers so the client sees a well-formed exchange.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line.trim().is_empty() => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    let _ = tx.send(path.clone());

    // Route on the path WITHOUT its query string, so a test can register
    // "/cdn/track.flac" and still be hit by a signed, query-carrying URL.
    let bare = path.split('?').next().unwrap_or(&path);
    let (status, body) = routes
        .get(bare)
        .cloned()
        .unwrap_or((404, b"not found".to_vec()));

    let reason = if status == 200 { "OK" } else { "Error" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

// ─── Fixtures ─────────────────────────────────────────────────────────────────

/// A minimal but VALID FLAC stream: the `fLaC` marker plus a single, final
/// STREAMINFO block. It carries no audio frames, which is fine — the pipeline
/// only ever writes tags to it and never decodes it — and it is enough for lofty
/// to identify the container and create a Vorbis comment block.
fn minimal_flac() -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    // Metadata block header: last-block flag (0x80) | type 0 (STREAMINFO), then
    // a 24-bit big-endian length of 34.
    out.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);

    let mut info = vec![0u8; 34];
    info[0..2].copy_from_slice(&4096u16.to_be_bytes()); // min block size
    info[2..4].copy_from_slice(&4096u16.to_be_bytes()); // max block size
                                                        // Sample rate 44100 (20 bits), then channels-1 (3 bits) and bits-per-sample-1
                                                        // (5 bits), packed across bytes 10..13 as the FLAC spec lays STREAMINFO out.
    let sample_rate: u32 = 44_100;
    info[10] = (sample_rate >> 12) as u8;
    info[11] = ((sample_rate >> 4) & 0xFF) as u8;
    info[12] = (((sample_rate & 0x0F) as u8) << 4) | (1 << 1); // ch-1 = 1 → stereo
    info[13] = 0xF0; // bits-per-sample-1 = 15 → 16-bit
    out.extend_from_slice(&info);
    out
}

/// A FLAC that arrives carrying an **ID3v2** tag in front of the `fLaC` marker.
///
/// This is not a contrived shape — plenty of tooling stamps ID3v2 onto FLAC, and
/// lofty reads it — and it is the shape that exposes the worst failure mode this
/// pipeline had. See `a_flac_carrying_id3v2_is_still_tagged`.
fn flac_with_leading_id3v2() -> Vec<u8> {
    // ID3v2.4 header: "ID3", version 4.0, no flags, then a SYNCSAFE size (7 bits
    // per byte) covering the frames that follow.
    let mut frame = b"TIT2".to_vec();
    let text = b"\x03OldTitle"; // 0x03 = UTF-8 encoding byte
    frame.extend_from_slice(&(text.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]); // frame flags
    frame.extend_from_slice(text);

    let size = frame.len() as u32;
    let syncsafe = [
        ((size >> 21) & 0x7F) as u8,
        ((size >> 14) & 0x7F) as u8,
        ((size >> 7) & 0x7F) as u8,
        (size & 0x7F) as u8,
    ];

    let mut out = b"ID3".to_vec();
    out.extend_from_slice(&[0x04, 0x00, 0x00]);
    out.extend_from_slice(&syncsafe);
    out.extend_from_slice(&frame);
    out.extend_from_slice(&minimal_flac());
    out
}

/// Opaque bytes for the asset routes, where only the round trip matters.
fn fake_jpeg() -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0]; // a JPEG SOI, so it is at least plausible
    out.extend_from_slice(b"qbz-test-cover-bytes");
    out
}

/// A catalog track built through the real deserializer, so the harness exercises
/// the same mapping the download path does rather than a hand-made struct.
fn catalog_track(json: &str) -> Track {
    serde_json::from_str(json).expect("catalog track fixture deserializes")
}

fn goody(name: &str, url: String) -> Goody {
    Goody {
        id: 5,
        name: name.to_string(),
        url,
        original_url: String::new(),
        file_url: None,
        file_format_id: Some(21),
        description: None,
    }
}

fn open_db(dir: &std::path::Path) -> LibraryDatabase {
    LibraryDatabase::open(&dir.join("library.db")).expect("open a temp library db")
}

/// Install the process-level rustls `CryptoProvider`, once.
///
/// Not optional and not TLS-specific in effect: reqwest is built with
/// `rustls-tls-webpki-roots-no-provider`, so **constructing any client panics
/// with "No provider set"** until a provider is installed — even for a plain
/// `http://` URL, because the TLS backend is initialised at builder time rather
/// than at connect time. The application does this at startup
/// (`qbz_app::ensure_crypto_provider`); a test binary has no startup, so it does
/// it here. Worth knowing beyond this file: any future tool or test that reaches
/// for `download_audio` without an app around it will hit the same panic.
fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn rt() -> tokio::runtime::Runtime {
    ensure_crypto_provider();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a test runtime")
}

// ─── The tests ────────────────────────────────────────────────────────────────

/// The asset fetcher moves real bytes over a real socket, unchanged.
#[test]
fn asset_bytes_round_trip_over_a_real_socket() {
    let cover = fake_jpeg();
    let mut routes = HashMap::new();
    routes.insert("/img/cover.jpg".to_string(), (200u16, cover.clone()));
    let server = FixtureServer::start(routes);

    let got = rt().block_on(svc::fetch_asset_bytes(&server.url("/img/cover.jpg")));
    assert_eq!(got.as_deref(), Some(cover.as_slice()));
    assert_eq!(server.requested_paths(), vec!["/img/cover.jpg".to_string()]);
}

/// Every failure mode of the asset fetcher degrades to `None`. This is the
/// property the whole scope expansion rests on: a missing cover or goodie must
/// never fail a download that already succeeded.
#[test]
fn asset_failures_never_raise() {
    let mut routes = HashMap::new();
    routes.insert("/empty".to_string(), (200u16, Vec::new()));
    routes.insert("/boom".to_string(), (500u16, b"server on fire".to_vec()));
    let server = FixtureServer::start(routes);

    let runtime = rt();
    assert_eq!(
        runtime.block_on(svc::fetch_asset_bytes(&server.url("/empty"))),
        None
    );
    assert_eq!(
        runtime.block_on(svc::fetch_asset_bytes(&server.url("/boom"))),
        None
    );
    assert_eq!(
        runtime.block_on(svc::fetch_asset_bytes(&server.url("/nope"))),
        None
    );
    assert_eq!(runtime.block_on(svc::fetch_asset_bytes("")), None);
    // Nothing is listening on port 1; the connect timeout bounds the attempt.
    assert_eq!(
        runtime.block_on(svc::fetch_asset_bytes("http://127.0.0.1:1/x")),
        None
    );
}

/// `cover.jpg` / `back.jpg` land beside the tracks; an absent one writes nothing.
#[test]
fn cover_files_are_written_beside_the_tracks() {
    let tmp = tempfile::tempdir().unwrap();
    let album_dir = tmp.path().join("Artist").join("Album");
    std::fs::create_dir_all(&album_dir).unwrap();

    let cover = fake_jpeg();
    svc::write_album_cover_files(&album_dir, Some(&cover), None, None);
    assert_eq!(std::fs::read(album_dir.join("cover.jpg")).unwrap(), cover);
    assert!(
        !album_dir.join("back.jpg").exists(),
        "an absent back cover must write nothing"
    );
    assert!(!album_dir.join("large_cover.jpg").exists());

    let back = b"back-cover-bytes".to_vec();
    let large = b"large-cover-bytes".to_vec();
    svc::write_album_cover_files(&album_dir, Some(&cover), Some(&back), Some(&large));
    assert_eq!(std::fs::read(album_dir.join("back.jpg")).unwrap(), back);
    assert_eq!(
        std::fs::read(album_dir.join("large_cover.jpg")).unwrap(),
        large,
        "the master-size cover lands under the desktop client's name"
    );

    // Empty slices count as absent, not as 0-byte files.
    let empty_dir = tmp.path().join("empty");
    std::fs::create_dir_all(&empty_dir).unwrap();
    svc::write_album_cover_files(&empty_dir, Some(&[]), Some(&[]), Some(&[]));
    assert!(!empty_dir.join("cover.jpg").exists());
    assert!(!empty_dir.join("back.jpg").exists());
    assert!(!empty_dir.join("large_cover.jpg").exists());
}

/// A goodie is fetched and written into the album folder under a sanitized name.
#[test]
fn a_goodie_is_downloaded_into_the_album_folder() {
    let pdf = b"%PDF-1.4 fake booklet".to_vec();
    let mut routes = HashMap::new();
    routes.insert("/goodies/booklet.pdf".to_string(), (200u16, pdf.clone()));
    let server = FixtureServer::start(routes);

    let tmp = tempfile::tempdir().unwrap();
    let album_dir = tmp.path().join("Album");

    let path = rt()
        .block_on(svc::download_goodie(
            &goody("Digital Booklet", server.url("/goodies/booklet.pdf")),
            &album_dir,
        ))
        .expect("the goodie downloads");

    assert!(path.ends_with("Digital Booklet.pdf"), "got {path}");
    assert_eq!(std::fs::read(&path).unwrap(), pdf);
}

/// A goodie name cannot escape the album folder. The name is the least
/// trustworthy field on the least verified struct in the feature — its populated
/// shape has never been observed — so containment is asserted, not assumed.
#[test]
fn a_goodie_name_cannot_escape_the_album_folder() {
    let mut routes = HashMap::new();
    routes.insert("/g/x.pdf".to_string(), (200u16, b"x".to_vec()));
    let server = FixtureServer::start(routes);

    let tmp = tempfile::tempdir().unwrap();
    let album_dir = tmp.path().join("Album");
    let runtime = rt();

    for hostile in [
        "../../../../etc/passwd",
        "/etc/passwd",
        "..\\..\\windows\\system32",
        "a/b/c",
    ] {
        let path = runtime
            .block_on(svc::download_goodie(
                &goody(hostile, server.url("/g/x.pdf")),
                &album_dir,
            ))
            .expect("still downloads, just somewhere safe");
        assert_eq!(
            std::path::Path::new(&path).parent(),
            Some(album_dir.as_path()),
            "{hostile:?} escaped to {path}"
        );
    }
}

/// A goodie with no usable URL is skipped, never fatal.
#[test]
fn a_goodie_without_a_url_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        rt().block_on(svc::download_goodie(
            &goody("Nameless", "   ".to_string()),
            tmp.path()
        )),
        None
    );
}

/// THE end-to-end one: real bytes over a real socket land at the contract's
/// deterministic path, register under the REQUESTED format, and carry tags taken
/// from the API payload plus an embedded cover.
#[test]
fn a_downloaded_file_is_written_registered_and_tagged() {
    use lofty::prelude::*;

    let audio = minimal_flac();
    let cover = fake_jpeg();
    let mut routes = HashMap::new();
    routes.insert("/cdn/track.flac".to_string(), (200u16, audio.clone()));
    let server = FixtureServer::start(routes);

    let tmp = tempfile::tempdir().unwrap();
    let db = open_db(tmp.path());
    let dest = tmp.path().join("Downloads");

    // Fetch through the real client, exactly as the download path does.
    let data = rt()
        .block_on(svc::fetch_asset_bytes(&server.url("/cdn/track.flac")))
        .expect("the CDN stand-in serves the audio");
    assert_eq!(data, audio, "bytes survive the socket unchanged");

    // The write + registry tail, with the album id folded into the one write.
    // Requested format 27, SERVED format 6 — they are allowed to disagree.
    let file_path = svc::write_and_register_track(
        &db,
        4242,
        Some("alb-1"),
        27,
        &data,
        "Café Tacvba",
        "Ré",
        "[FLAC][24-bit,192kHz]",
        7,
        "El Ciclón",
        6,
        "audio/flac",
        dest.to_str().unwrap(),
    )
    .expect("write and register");

    // §4.4: deterministic path, Unicode survives, the quality suffix keeps its
    // ASCII brackets, and the `.part` file is gone.
    assert!(
        file_path.ends_with("Café Tacvba/Ré [FLAC][24-bit,192kHz]/07 - El Ciclón.flac"),
        "unexpected path: {file_path}"
    );
    assert!(!std::path::Path::new(&format!("{file_path}.part")).exists());

    // §3: the registry records the REQUESTED format, and `album_id` is set —
    // which is what makes the album-downloaded rule answerable at all.
    let formats = db.get_downloaded_purchase_formats().unwrap();
    assert!(
        formats.contains(&(4242, 27)),
        "registry must hold the REQUESTED format 27, not the served 6: {formats:?}"
    );
    let counts = db.get_downloaded_purchase_album_counts().unwrap();
    assert_eq!(counts.get("alb-1"), Some(&1));

    // §14.1: tags come from the API payload, including the per-track artist,
    // and the cover is embedded exactly once.
    let track = catalog_track(
        r#"{
            "id": 4242,
            "title": "El Ciclón",
            "version": "Live",
            "track_number": 7,
            "media_number": 1,
            "isrc": "MX1234567890",
            "performer": {"id": 1, "name": "Café Tacvba"},
            "album": {"id": "alb-1", "title": "Ré"},
            "copyright": "1994 WEA"
        }"#,
    );
    let ctx = svc::PurchaseAlbumContext {
        album_artist: "Café Tacvba".to_string(),
        year: Some(1994),
        genre: Some("Rock".to_string()),
        label: Some("WEA".to_string()),
        cover_jpeg: Some(cover),
    };
    svc::tag_downloaded_file(&file_path, &track, &ctx);

    let tagged = lofty::read_from_path(std::path::Path::new(&file_path)).expect("re-read the file");
    let tag = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .expect("a tag exists");

    assert_eq!(tag.title().as_deref(), Some("El Ciclón"));
    assert_eq!(
        tag.artist().as_deref(),
        Some("Café Tacvba"),
        "the per-track performer must be written outright — the editor's rename \
         rule reads the file, and a fresh download has nothing to read"
    );
    assert_eq!(tag.album().as_deref(), Some("Ré"));
    assert_eq!(tag.track(), Some(7));
    assert_eq!(tag.disk(), Some(1));
    assert_eq!(
        tag.pictures().len(),
        1,
        "exactly one embedded cover, never a second copy"
    );

    // Tagging twice must not accumulate art — the guard that makes a retry safe.
    svc::tag_downloaded_file(&file_path, &track, &ctx);
    let retagged = lofty::read_from_path(std::path::Path::new(&file_path)).unwrap();
    let tag2 = retagged
        .primary_tag()
        .or_else(|| retagged.first_tag())
        .unwrap();
    assert_eq!(
        tag2.pictures().len(),
        1,
        "re-tagging must not duplicate the cover"
    );
}

/// THE regression test for the worst failure this pipeline had, and the reason
/// the harness above was not enough on its own.
///
/// The tag writer used to pick its target tag as "the primary one, or else the
/// first one I can find". For a FLAC that arrives carrying an ID3v2 tag, the
/// primary (VorbisComments) is absent while the first is that ID3v2 — so every
/// field went into a tag type lofty **refuses to write** for FLAC. And it
/// refuses silently: the save loop skips non-writable tags with `continue`
/// rather than erroring, so the call returned `Ok(())`, nothing was logged, and
/// the file came out with none of its metadata. On a feature nobody here can
/// smoke-test, that is a bug that reaches users untouched.
///
/// The clean FLAC fixture used everywhere else in this file cannot catch it,
/// because with NO tag at all both accessors return `None` and the correct
/// branch runs by luck.
#[test]
fn a_flac_carrying_id3v2_is_still_tagged() {
    use lofty::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("with-id3.flac");
    std::fs::write(&file, flac_with_leading_id3v2()).unwrap();

    // Sanity: the fixture really does present an ID3v2 tag, and really does NOT
    // present the Vorbis comments that FLAC actually writes. Without this the
    // test could pass while proving nothing.
    let before = lofty::read_from_path(&file).expect("the fixture is a readable FLAC");
    assert!(
        before.tag(lofty::tag::TagType::Id3v2).is_some(),
        "the fixture must carry an ID3v2 tag, or it does not exercise the bug"
    );
    assert!(
        before.tag(lofty::tag::TagType::VorbisComments).is_none(),
        "the fixture must NOT already have Vorbis comments"
    );

    let track = catalog_track(
        r#"{"id":1,"title":"New Title","track_number":3,
            "performer":{"id":1,"name":"The Artist"},
            "album":{"id":"a","title":"The Album"}}"#,
    );
    let ctx = svc::PurchaseAlbumContext {
        album_artist: "The Artist".to_string(),
        year: Some(2001),
        genre: None,
        label: None,
        cover_jpeg: None,
    };
    svc::tag_downloaded_file(&file.to_string_lossy(), &track, &ctx);

    let after = lofty::read_from_path(&file).expect("re-read");
    let vorbis = after
        .tag(lofty::tag::TagType::VorbisComments)
        .expect("the tags must land in VORBIS COMMENTS — the type FLAC can actually write");

    assert_eq!(vorbis.title().as_deref(), Some("New Title"));
    assert_eq!(vorbis.artist().as_deref(), Some("The Artist"));
    assert_eq!(vorbis.album().as_deref(), Some("The Album"));
    assert_eq!(vorbis.track(), Some(3));
}

/// A tagging failure must never be able to fail a download: the function returns
/// nothing, so a caller cannot propagate it even by accident.
#[test]
fn tagging_a_missing_or_unreadable_file_is_survivable() {
    let tmp = tempfile::tempdir().unwrap();
    let track = catalog_track(r#"{"id":1,"title":"T","track_number":1}"#);
    let ctx = svc::PurchaseAlbumContext::default();

    // Missing file.
    svc::tag_downloaded_file(
        &tmp.path().join("nope.flac").to_string_lossy(),
        &track,
        &ctx,
    );

    // Present but not audio.
    let junk = tmp.path().join("junk.flac");
    std::fs::write(&junk, b"this is not a flac stream").unwrap();
    svc::tag_downloaded_file(&junk.to_string_lossy(), &track, &ctx);

    // Reaching here at all is the assertion: neither call panicked or returned.
    assert!(junk.exists(), "the file is left exactly as it was");
}
