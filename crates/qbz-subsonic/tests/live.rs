//! LIVE tests against a real Subsonic-compatible server. Skipped unless the
//! environment names one.
//!
//!   QBZ_SUBSONIC_URL=http://192.168.0.69:4533 \
//!   QBZ_SUBSONIC_USER=admin QBZ_SUBSONIC_PASS=... \
//!   cargo test -p qbz-subsonic --test live -- --nocapture
//!
//! Unlike Jellyfin's, these run happily in PARALLEL: Subsonic auth is
//! stateless — credentials ride on every request and there is no server-side
//! session to invalidate — so nothing here can revoke anything else's token.
//! That difference is itself worth knowing, and it is why this file has no
//! shared-session machinery.

use std::env;
use std::sync::Once;

use qbz_subsonic::{Credentials, SubsonicClient, SubsonicError, SweepMode};

fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn server() -> Option<(String, String, String)> {
    let v = (
        env::var("QBZ_SUBSONIC_URL").ok()?,
        env::var("QBZ_SUBSONIC_USER").ok()?,
        env::var("QBZ_SUBSONIC_PASS").ok()?,
    );
    install_crypto_provider();
    Some(v)
}

macro_rules! skip_without_server {
    () => {
        match server() {
            Some(v) => v,
            None => {
                eprintln!("SKIP: set QBZ_SUBSONIC_URL / _USER / _PASS to run the live tests");
                return;
            }
        }
    };
}

fn connect(url: &str, user: &str, pass: &str) -> SubsonicClient {
    // A FIXED salt, as the app uses one — see `Credentials`' docs for why a
    // rolling salt would re-download every cover on every pass.
    SubsonicClient::new(url, Credentials::new(user, pass, "qbzlive")).unwrap()
}

#[tokio::test]
async fn ping_reports_an_opensubsonic_server() {
    let (url, user, pass) = skip_without_server!();
    let info = connect(&url, &user, &pass).ping().await.expect("ping");
    eprintln!(
        "server: {} {} (openSubsonic={})",
        info.kind,
        info.server_version.as_deref().unwrap_or(&info.version),
        info.open_subsonic
    );
    // Without the OpenSubsonic extensions there is no bitDepth / samplingRate
    // and every quality badge in the app degrades to unknown. Worth asserting
    // on the bench so a server downgrade is loud.
    assert!(info.open_subsonic, "this server is not OpenSubsonic");
}

/// THE TRAP, end to end. A wrong password is answered with **HTTP 200** and a
/// failure envelope; only the envelope parser can tell. If this ever starts
/// passing as success, every other assertion in the suite is worthless.
#[tokio::test]
async fn wrong_credentials_fail_despite_an_http_200() {
    let (url, user, _) = skip_without_server!();
    let c = SubsonicClient::new(
        &url,
        Credentials::new(&user, "definitely-not-the-password", "qbzlive"),
    )
    .unwrap();
    match c.ping().await {
        Err(SubsonicError::Unauthorized) => {}
        other => panic!("expected Unauthorized, got {other:?}"),
    }

    // And the raw truth underneath it: the transport really did say 200.
    let bad = Credentials::new(&user, "definitely-not-the-password", "qbzlive");
    let raw = format!(
        "{}/ping.view?{}",
        qbz_subsonic::normalize_base_url(&url),
        bad.query()
    );
    let status = reqwest::get(&raw).await.expect("raw ping").status();
    assert_eq!(
        status.as_u16(),
        200,
        "the protocol stopped answering 200 on failure — the envelope guard may be redundant now"
    );
}

#[tokio::test]
async fn the_fast_sweep_is_detected_and_paginates() {
    let (url, user, pass) = skip_without_server!();
    let c = connect(&url, &user, &pass);
    let mode = c.detect_sweep_mode().await;
    eprintln!("sweep mode: {mode:?}");
    assert_eq!(mode, SweepMode::Search3, "Navidrome should honour search3");

    let first = c.search_page(0).await.expect("page 0");
    assert!(!first.is_empty());
    let second = c.search_page(500).await.expect("page 500");
    assert!(!second.is_empty(), "the second page came back empty");
    // Real pagination, not the same page twice.
    assert_ne!(first[0].id, second[0].id, "songOffset did not move the window");

    let hires = first
        .iter()
        .filter(|t| t.bit_depth.unwrap_or(0) > 16 || t.sample_rate_hz.unwrap_or(0) > 48_000)
        .count();
    let with_depth = first.iter().filter(|t| t.bit_depth.is_some()).count();
    assert!(
        with_depth > 0,
        "no row carried a bit depth — this server is not returning OpenSubsonic fields"
    );
    for t in first.iter().filter(|t| t.suffix == "mp3") {
        assert_eq!(t.bit_depth, None, "an mp3 reported a bit depth: {}", t.title);
    }
    eprintln!(
        "page 0: {} rows, {} with a bit depth, {} hi-res",
        first.len(),
        with_depth,
        hires
    );
}

/// The PORTABLE path has to work too — it is the fallback for every server
/// that was never on the bench.
#[tokio::test]
async fn the_per_album_sweep_also_returns_tracks() {
    let (url, user, pass) = skip_without_server!();
    let c = connect(&url, &user, &pass);
    let ids = c.album_ids(0).await.expect("album ids");
    assert!(!ids.is_empty(), "no albums");
    let tracks = c.album_tracks(&ids[0]).await.expect("album tracks");
    assert!(!tracks.is_empty(), "album {} has no tracks", ids[0]);
    for t in &tracks {
        assert!(!t.id.is_empty());
        assert!(!t.album_id.is_empty(), "track {} has no album id", t.title);
    }
    // getAlbumList2 caps at 500 whatever `size` asks for.
    assert!(ids.len() <= 500, "the server returned {} albums", ids.len());
    eprintln!("per-album: {} album ids, first album has {} tracks", ids.len(), tracks.len());
}

/// THE audio contract. `format=raw` must hand back the ORIGINAL bytes, and the
/// server's own reported `size` is an independent check that nothing was
/// re-encoded on the way out.
#[tokio::test]
async fn the_stream_is_raw_seekable_and_the_reported_size() {
    let (url, user, pass) = skip_without_server!();
    let c = connect(&url, &user, &pass);
    let page = c.search_page(0).await.expect("page");
    let track = page
        .iter()
        .find(|t| t.suffix == "flac" && t.size.is_some())
        .expect("no flac with a reported size in the first page");

    let resp = reqwest::Client::new()
        .get(c.stream_url(&track.id))
        .header("Range", "bytes=0-65535")
        .send()
        .await
        .expect("range request");
    assert_eq!(resp.status().as_u16(), 206, "the server refused a Range request");
    let range = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.bytes().await.expect("body");

    assert!(
        qbz_subsonic::looks_like_audio(Some(&ctype), bytes.len()),
        "the body looks like an error envelope, not audio: {ctype}"
    );
    assert_eq!(&bytes[..4], b"fLaC", "not a FLAC bitstream");
    // `Content-Range: bytes 0-65535/<total>` — the total must be the size the
    // API reported for the file. A transcode could not match it.
    let total: u64 = range
        .rsplit('/')
        .next()
        .and_then(|t| t.parse().ok())
        .expect("no total in Content-Range");
    assert_eq!(
        total,
        track.size.unwrap(),
        "streamed length differs from the reported file size — the server re-encoded"
    );
    eprintln!("stream: {} -> 206 {ctype}, total {total} == reported size", track.title);
}

/// Cover art works WITH credentials, and — the trap again — an unauthenticated
/// request answers 200 with something that is not an image.
#[tokio::test]
async fn cover_art_needs_credentials_and_says_so_with_a_200() {
    let (url, user, pass) = skip_without_server!();
    let c = connect(&url, &user, &pass);
    let page = c.search_page(0).await.expect("page");
    let track = page
        .iter()
        .find(|t| t.cover_art.is_some())
        .expect("no row in the first page has cover art");
    let art = track.cover_art.as_deref().unwrap();

    let resp = reqwest::get(c.cover_url(art, qbz_subsonic::IMAGE_PX))
        .await
        .expect("cover");
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ctype.starts_with("image/"), "authenticated cover returned {ctype}");
    let len = resp.bytes().await.expect("cover body").len();
    assert!(len > 1024);

    // Same endpoint, no credentials: HTTP 200, and NOT an image.
    let naked = format!(
        "{}/getCoverArt.view?v={}&c=QBZ&id={}",
        qbz_subsonic::normalize_base_url(&url),
        qbz_subsonic::API_VERSION,
        art
    );
    let bad = reqwest::get(&naked).await.expect("naked cover");
    assert_eq!(bad.status().as_u16(), 200, "an unauthenticated cover was NOT a 200");
    let bad_ct = bad
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bad_body = bad.bytes().await.expect("naked body");
    assert!(
        !qbz_subsonic::looks_like_audio(Some(&bad_ct), bad_body.len()),
        "the guard failed to reject an error envelope"
    );
    eprintln!(
        "cover: authed {ctype} {len} B · unauthed HTTP 200 {bad_ct} {} B",
        bad_body.len()
    );
}
