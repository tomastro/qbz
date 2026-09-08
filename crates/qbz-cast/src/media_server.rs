//! Local HTTP server for casting media streaming
//!
//! Provides a simple HTTP server that serves audio files to cast devices.
//! Supports byte-range requests for seeking.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::CastError;

/// DLNA `contentFeatures.dlna.org` value advertised on GET/HEAD responses.
///
/// `DLNA.ORG_OP=01` = byte-range seek supported, no time-seek.
/// `DLNA.ORG_FLAGS=01700000...` = streaming/interactive-transfer flags.
/// No `DLNA.ORG_PN` — FLAC has no standard DLNA profile name.
///
/// Strict renderers (e.g. HQPlayer) HEAD-probe the URL and treat the stream as
/// invalid/finished without these headers. Kept in sync with the DIDL
/// `protocolInfo` in `dlna::device::build_didl_metadata`.
pub const DLNA_CONTENT_FEATURES: &str =
    "DLNA.ORG_OP=01;DLNA.ORG_FLAGS=01700000000000000000000000000000";

/// Chunked-encoding threshold for media responses.
///
/// tiny_http 0.12.0 auto-selects `Transfer-Encoding: chunked` (dropping
/// `Content-Length`) once a body reaches its default 32 KiB threshold. Strict
/// DLNA renderers (e.g. HQPlayer) reject a chunked media body: they play the
/// few seconds they buffer, then go STOPPED without range-continuing. Raising
/// the threshold above any real file size forces Identity encoding with an
/// explicit `Content-Length` on every media response (200, 206, and HEAD).
const NO_CHUNK_THRESHOLD: usize = usize::MAX;

#[derive(Clone)]
struct MediaEntry {
    content_type: String,
    size: u64,
    source: MediaSource,
}

#[derive(Clone)]
enum MediaSource {
    // Arc so cloning an entry out of the map for an in-flight request never
    // copies the whole track, and so eviction can't invalidate a response
    // that is still being served (#550).
    Data(std::sync::Arc<Vec<u8>>),
    File(PathBuf),
    /// Bytes that are still ARRIVING while they are served: a Qobuz track
    /// being downloaded, or a Plex/Jellyfin/Subsonic stream proxied from its
    /// server. The total size is known up front (advertised as
    /// Content-Length); reads past the buffered edge block until the bytes
    /// exist. This is what turns "download 100 MB, then play" into "play
    /// after the first segment".
    Reader(Arc<dyn RangeSource>),
}

/// A range-addressable byte source whose bytes may still be arriving.
/// `open` returns a reader positioned at `start` that yields at most `len`
/// bytes and BLOCKS (never errors) while a requested byte is not there yet.
pub trait RangeSource: Send + Sync {
    fn open(&self, start: u64, len: u64) -> std::io::Result<Box<dyn Read + Send>>;
}

/// Simple HTTP server for audio streaming to cast devices
pub struct MediaServer {
    port: u16,
    base_url: String,
    /// Per-session random token embedded in the served path
    /// (`/audio/<token>/<id>`). Raises the bar from "guess a small integer id"
    /// to "guess a 128-bit token" for a LAN peer trying to pull buffered bytes.
    /// Carried in the PATH (not a query string) because some DLNA renderers
    /// drop query strings.
    token: String,
    entries: Arc<Mutex<HashMap<u64, MediaEntry>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MediaServer {
    /// Start server on available port
    /// Uses port 9876 by default for DLNA compatibility, falls back to random port if busy
    pub fn start() -> Result<Self, CastError> {
        // Try fixed port first for easier firewall configuration
        let server = Server::http("0.0.0.0:9876")
            .or_else(|_| {
                log::warn!("MediaServer: Port 9876 busy, using random port");
                Server::http("0.0.0.0:0")
            })
            .map_err(|e| CastError::Server(format!("Failed to start HTTP server: {}", e)))?;

        let port = server
            .server_addr()
            .to_ip()
            .map(|addr| addr.port())
            .ok_or_else(|| CastError::Server("Failed to determine HTTP port".to_string()))?;

        let base_ip = local_ip().unwrap_or_else(|| IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        let base_url = format_base_url(base_ip, port);

        // Do not log the full base_url once the path token is embedded in
        // request paths — host:port is enough for diagnostics.
        log::info!("MediaServer: Started on {base_ip}:{port}");

        let entries = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let token = generate_token();

        let entries_clone = entries.clone();
        let shutdown_clone = shutdown.clone();
        let token_clone = token.clone();
        let port_for_log = port;

        let handle = thread::spawn(move || {
            log::info!(
                "MediaServer: Thread started, listening on port {}",
                port_for_log
            );
            while !shutdown_clone.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(250)) {
                    Ok(Some(request)) => {
                        // ONE THREAD PER REQUEST. A media body is written at
                        // the renderer's pace (a 100 MB track takes the whole
                        // song to drain, and a progressive source blocks on
                        // bytes that have not arrived), so serving inline
                        // stalled every other request — the HEAD probe strict
                        // renderers send before Play, a second Range request
                        // for a seek — for the duration of the track.
                        let entries = entries_clone.clone();
                        let token = token_clone.clone();
                        thread::spawn(move || {
                            let response = handle_request(
                                request.method(),
                                request.url(),
                                &request,
                                &entries,
                                &token,
                            );
                            let _ = request.respond(response);
                        });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::error!("MediaServer: Thread error: {:?}, exiting", e);
                        break;
                    }
                }
            }
            log::info!("MediaServer: Thread exiting");
        });

        Ok(Self {
            port,
            base_url,
            token,
            entries,
            shutdown,
            handle: Some(handle),
        })
    }

    /// Stop server
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    /// Get base URL (e.g., "http://192.168.1.100:8080")
    /// Drop every registered entry (releases the full-track buffers). Called
    /// on cast stop/disconnect so track bytes don't outlive the session (#550).
    pub fn clear_entries(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// Relative served path for an id, including the session token.
    fn audio_path(&self, id: u64) -> String {
        format!("/audio/{}/{}", self.token, id)
    }

    /// Register audio data to serve (returns path like "/audio/<token>/123")
    pub fn register_audio(&mut self, id: u64, data: Vec<u8>, content_type: &str) -> String {
        self.register_audio_shared(id, Arc::new(data), content_type)
    }

    /// Same, for bytes another consumer keeps alive as well (no copy).
    pub fn register_audio_shared(
        &mut self,
        id: u64,
        data: Arc<Vec<u8>>,
        content_type: &str,
    ) -> String {
        let entry = MediaEntry {
            content_type: content_type.to_string(),
            size: data.len() as u64,
            source: MediaSource::Data(data),
        };

        if let Ok(mut entries) = self.entries.lock() {
            // Only the track being cast is servable — evict everything else.
            // Entries held full track bytes and NOTHING ever removed them, so
            // a DLNA session grew by one full track per auto-advance until app
            // exit (#550: 50 MB -> 4.5 GB). An in-flight response keeps its
            // own Arc, so eviction never truncates an ongoing range request.
            entries.retain(|k, _| *k == id);
            entries.insert(id, entry);
        }

        self.audio_path(id)
    }

    /// Register a range-addressable source whose bytes may still be arriving
    /// (see [`RangeSource`]). `size` is the FINAL byte length — it is what the
    /// renderer is told in Content-Length, so it must be exact.
    pub fn register_reader(
        &mut self,
        id: u64,
        size: u64,
        content_type: &str,
        source: Arc<dyn RangeSource>,
    ) -> String {
        let entry = MediaEntry {
            content_type: content_type.to_string(),
            size,
            source: MediaSource::Reader(source),
        };
        if let Ok(mut entries) = self.entries.lock() {
            // Same eviction rule as register_audio (#550).
            entries.retain(|k, _| *k == id);
            entries.insert(id, entry);
        }
        self.audio_path(id)
    }

    /// Register a local file to serve, inferring the content type from the
    /// extension via [`content_type_for_path`].
    pub fn register_file(&mut self, id: u64, file_path: &str) -> Result<String, CastError> {
        let path = Path::new(file_path);
        let content_type = content_type_for_path(path);
        self.register_file_with_content_type(id, file_path, &content_type)
    }

    /// Register a local file to serve with an explicit content type. Lets the
    /// caller supply a richer MIME than extension-sniffing yields (e.g. from
    /// decoded track metadata). Streams the file from disk (range-capable),
    /// never reads the whole file into RAM.
    pub fn register_file_with_content_type(
        &mut self,
        id: u64,
        file_path: &str,
        content_type: &str,
    ) -> Result<String, CastError> {
        let path = Path::new(file_path);
        if !path.exists() {
            return Err(CastError::InvalidRequest(format!(
                "File not found: {}",
                file_path
            )));
        }

        let size = path.metadata().map_err(CastError::Io)?.len();

        let entry = MediaEntry {
            content_type: content_type.to_string(),
            size,
            source: MediaSource::File(path.to_path_buf()),
        };

        if let Ok(mut entries) = self.entries.lock() {
            // Same eviction rule as register_audio (#550): switching to a
            // local file must also release the previous track's bytes.
            entries.retain(|k, _| *k == id);
            entries.insert(id, entry);
        }

        Ok(self.audio_path(id))
    }

    /// Get full URL for registered audio
    pub fn get_audio_url(&self, id: u64) -> Option<String> {
        let entries = self.entries.lock().ok()?;
        if entries.contains_key(&id) {
            return Some(format!("{}{}", self.base_url, self.audio_path(id)));
        }
        None
    }

    /// Get full URL for registered audio using the local IP that can reach target_ip.
    pub fn get_audio_url_for_target(&self, id: u64, target_ip: &str) -> Option<String> {
        let entries = self.entries.lock().ok()?;
        if !entries.contains_key(&id) {
            return None;
        }

        let base_url = local_ip_for_target(target_ip)
            .map(|ip| format_base_url(ip, self.port))
            .unwrap_or_else(|| self.base_url.clone());

        Some(format!("{}{}", base_url, self.audio_path(id)))
    }

    /// Get server port
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle_request(
    method: &Method,
    url: &str,
    request: &tiny_http::Request,
    entries: &Arc<Mutex<HashMap<u64, MediaEntry>>>,
    expected_token: &str,
) -> Response<Box<dyn Read>> {
    log::info!(
        "MediaServer: {} request from {:?} for {}",
        method,
        request.remote_addr(),
        redact_media_uri(url)
    );

    // Allow GET and HEAD. Strict DLNA renderers HEAD-probe the URL to validate
    // the resource before playing; 405-rejecting HEAD makes them treat the
    // stream as invalid and stop shortly after buffering.
    let is_head = method == &Method::Head;
    if method != &Method::Get && !is_head {
        log::warn!("MediaServer: Rejected unsupported method: {}", method);
        return empty_response(405);
    }

    let (token, id) = match parse_audio_request(url) {
        Some(parsed) => parsed,
        None => {
            log::warn!(
                "MediaServer: 404 - Could not parse audio request from URL: {}",
                redact_media_uri(url)
            );
            return empty_response(404);
        }
    };

    // Constant-ish token check — reject LAN peers that don't hold the token.
    if token != expected_token {
        log::warn!(
            "MediaServer: 403 - token mismatch for URL: {}",
            redact_media_uri(url)
        );
        return empty_response(403);
    }

    let entry = match entries.lock().ok().and_then(|map| map.get(&id).cloned()) {
        Some(entry) => {
            log::info!(
                "MediaServer: Found entry for ID {}, content-type: {}, size: {} bytes",
                id,
                entry.content_type,
                entry.size
            );
            entry
        }
        None => {
            log::warn!("MediaServer: 404 - No entry found for ID: {}", id);
            return empty_response(404);
        }
    };

    // HEAD: advertise the resource (type/size/DLNA features) without reading
    // any bytes. tiny_http suppresses the body for HEAD while still emitting the
    // advertised Content-Length.
    if is_head {
        let headers = vec![
            header("Content-Type", &entry.content_type),
            header("Accept-Ranges", "bytes"),
            header("transferMode.dlna.org", "Streaming"),
            header("contentFeatures.dlna.org", DLNA_CONTENT_FEATURES),
        ];
        return Response::new(
            StatusCode(200),
            headers,
            Box::new(std::io::Cursor::new(Vec::new())) as Box<dyn Read>,
            Some(entry.size as usize),
            None,
        )
        .with_chunked_threshold(NO_CHUNK_THRESHOLD);
    }

    let range_header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str());

    let range = range_header.and_then(|header| parse_range(header, entry.size));

    let (body, body_len, status_code, content_range) = match open_range(&entry, range) {
        Ok(result) => result,
        Err(_) => return empty_response(500),
    };

    let mut response = Response::new(
        status_code,
        vec![
            header("Content-Type", &entry.content_type),
            header("Accept-Ranges", "bytes"),
            header("transferMode.dlna.org", "Streaming"),
            header("contentFeatures.dlna.org", DLNA_CONTENT_FEATURES),
        ],
        body,
        Some(body_len),
        None,
    )
    .with_chunked_threshold(NO_CHUNK_THRESHOLD);

    if let Some(content_range) = content_range {
        response = response.with_header(header("Content-Range", &content_range));
    }

    response
}

/// Empty-body error response in the boxed-reader shape `handle_request`
/// returns.
fn empty_response(code: u16) -> Response<Box<dyn Read>> {
    Response::new(
        StatusCode(code),
        Vec::new(),
        Box::new(std::io::Cursor::new(Vec::new())),
        Some(0),
        None,
    )
}

/// Open the requested byte range as a STREAMING reader.
///
/// File entries are served straight from disk (seek + bounded `Take`): the
/// range is never materialized in RAM first. The old read-into-Vec path
/// stalled the response for seconds on hi-res local files (a 32/384 PCM or
/// DSD track is 0.7-1.4 GB) — long enough for a strict renderer to time the
/// fetch out (#646) — spiked RAM by the full file size per request, and
/// blocked this single-threaded server for the whole copy. In-memory
/// (Qobuz-stream) entries keep the existing bounded slice copy.
fn open_range(
    entry: &MediaEntry,
    range: Option<(u64, u64)>,
) -> Result<(Box<dyn Read>, usize, StatusCode, Option<String>), std::io::Error> {
    // 0-byte entries 500'd under the old read-into-Vec path (read_exact of 1
    // byte past EOF); keep that instead of advertising a lying Content-Length.
    if entry.size == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "empty media entry",
        ));
    }
    let (start, end, status_code, content_range) = if let Some((start, end)) = range {
        let content_range = format!("bytes {}-{}/{}", start, end, entry.size);
        (start, end, StatusCode(206), Some(content_range))
    } else {
        (0, entry.size.saturating_sub(1), StatusCode(200), None)
    };

    match &entry.source {
        MediaSource::Data(data) => {
            let end = end.min(entry.size.saturating_sub(1)) as usize;
            let start = start.min(end as u64) as usize;
            let buf = data[start..=end].to_vec();
            let len = buf.len();
            Ok((
                Box::new(std::io::Cursor::new(buf)),
                len,
                status_code,
                content_range,
            ))
        }
        MediaSource::File(path) => {
            let mut file = File::open(path)?;
            file.seek(SeekFrom::Start(start))?;
            let length = end - start + 1;
            Ok((
                Box::new(file.take(length)),
                length as usize,
                status_code,
                content_range,
            ))
        }
        MediaSource::Reader(source) => {
            let end = end.min(entry.size.saturating_sub(1));
            let start = start.min(end);
            let length = end - start + 1;
            let reader = source.open(start, length)?;
            Ok((reader, length as usize, status_code, content_range))
        }
    }
}

/// Parse `/audio/<token>/<id>` into `(token, id)`. The token is carried in the
/// path (not a query string) so DLNA renderers that strip query strings still
/// authenticate.
fn parse_audio_request(url: &str) -> Option<(String, u64)> {
    let path = url.split('?').next().unwrap_or(url);
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() != 3 || parts[0] != "audio" {
        return None;
    }
    let id = parts[2].parse().ok()?;
    Some((parts[1].to_string(), id))
}

fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if !header.starts_with("bytes=") {
        return None;
    }

    let range = header.trim_start_matches("bytes=");
    let mut parts = range.split('-');
    let start_str = parts.next().unwrap_or("");
    let end_str = parts.next().unwrap_or("");

    if start_str.is_empty() {
        let suffix = end_str.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = total.saturating_sub(suffix);
        let end = total.saturating_sub(1);
        return Some((start, end));
    }

    let start = start_str.parse::<u64>().ok()?;
    if start >= total {
        return None;
    }

    let end = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        end_str.parse::<u64>().ok()?.min(total.saturating_sub(1))
    };

    Some((start, end))
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).unwrap()
}

fn local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

fn local_ip_for_target(target_ip: &str) -> Option<IpAddr> {
    let ip: IpAddr = target_ip.parse().ok()?;
    let bind_addr = if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let socket = UdpSocket::bind(bind_addr).ok()?;
    socket.connect(SocketAddr::new(ip, 80)).ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

fn format_base_url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(addr) => format!("http://{}:{}", addr, port),
        IpAddr::V6(addr) => format!("http://[{}]:{}", addr, port),
    }
}

/// Map a local-file extension to a content type. Mirrors the rich map the
/// Tauri local-cast path used (`local_track_content_type`) so casting a local
/// file advertises the right MIME — the previous map here was poorer
/// (`audio/ape`, no mp3/ogg/opus/aac, octet-stream gaps).
fn content_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("flac") => "audio/flac".to_string(),
        Some("wav") => "audio/wav".to_string(),
        Some("m4a") | Some("alac") | Some("mp4") => "audio/mp4".to_string(),
        Some("aiff") | Some("aif") => "audio/aiff".to_string(),
        // DSD containers — de-facto DLNA MIMEs (the MinimServer convention).
        // DLNA standardizes no DSD profile, so hi-res DSD is always
        // vendor-extension territory; HQPlayer-class renderers play native
        // DSF/DFF served with these types (#646). The old octet-stream
        // fallback made strict renderers reject the stream (AVTransport 714).
        Some("dsf") => "audio/x-dsf".to_string(),
        Some("dff") => "audio/x-dff".to_string(),
        Some("ape") => "audio/x-ape".to_string(),
        Some("mp3") => "audio/mpeg".to_string(),
        Some("ogg") | Some("oga") => "audio/ogg".to_string(),
        Some("opus") => "audio/opus".to_string(),
        Some("aac") => "audio/aac".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// Generate a 128-bit hex session token from OS CSPRNG entropy.
/// Embedded in the path (`/audio/<token>/<id>`) so LAN peers cannot guess
/// a sequential id alone.
fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Extremely rare; fall back to a non-crypto scramble so cast still works.
        use std::hash::BuildHasher;
        let hi = std::collections::hash_map::RandomState::new().hash_one(0x9E37_79B9u64);
        let lo = std::collections::hash_map::RandomState::new().hash_one(0x85EB_CA77u64);
        return format!("{:016x}{:016x}", hi, lo);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Redact the cast media path token in logs (`/audio/<token>/<id>` → `/audio/***/<id>`).
/// Session tokens must not appear in log bundles.
pub(crate) fn redact_media_uri(s: &str) -> String {
    if !s.contains("/audio/") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/audio/") {
        out.push_str(&rest[..i]);
        out.push_str("/audio/");
        rest = &rest[i + "/audio/".len()..];
        // token is the next path segment
        if let Some(slash) = rest.find('/') {
            out.push_str("***");
            out.push('/');
            rest = &rest[slash + 1..];
        } else {
            out.push_str("***");
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_audio_request_tokened_path() {
        assert_eq!(
            parse_audio_request("/audio/deadbeef/123"),
            Some(("deadbeef".to_string(), 123))
        );
        // Query string is ignored.
        assert_eq!(
            parse_audio_request("/audio/tok/42?x=1"),
            Some(("tok".to_string(), 42))
        );
        // Old un-tokened path no longer parses.
        assert_eq!(parse_audio_request("/audio/123"), None);
        assert_eq!(parse_audio_request("/other/tok/1"), None);
        assert_eq!(parse_audio_request("/audio/tok/notanum"), None);
    }

    #[test]
    fn content_type_map_is_rich() {
        use std::path::Path;
        assert_eq!(content_type_for_path(Path::new("a.flac")), "audio/flac");
        assert_eq!(content_type_for_path(Path::new("a.ape")), "audio/x-ape");
        assert_eq!(content_type_for_path(Path::new("a.mp3")), "audio/mpeg");
        assert_eq!(content_type_for_path(Path::new("a.opus")), "audio/opus");
        assert_eq!(content_type_for_path(Path::new("a.m4a")), "audio/mp4");
        // DSD containers get the de-facto DLNA MIMEs, not octet-stream (#646).
        assert_eq!(content_type_for_path(Path::new("a.dsf")), "audio/x-dsf");
        assert_eq!(content_type_for_path(Path::new("a.dff")), "audio/x-dff");
        assert_eq!(
            content_type_for_path(Path::new("a.xyz")),
            "application/octet-stream"
        );
    }

    #[test]
    fn redact_media_uri_hides_token() {
        assert_eq!(
            redact_media_uri("http://192.168.1.2:9876/audio/deadbeefcafebabe/42"),
            "http://192.168.1.2:9876/audio/***/42"
        );
        assert_eq!(
            redact_media_uri("/audio/tokentokentoken/7?x=1"),
            "/audio/***/7?x=1"
        );
        let raw = "/audio/aabbccddeeff0011/9";
        let red = redact_media_uri(raw);
        assert!(!red.contains("aabbccddeeff0011"));
        assert!(red.contains("***/9"));
    }

    struct SliceSource(Vec<u8>);
    impl RangeSource for SliceSource {
        fn open(&self, start: u64, len: u64) -> std::io::Result<Box<dyn Read + Send>> {
            let start = start as usize;
            let end = (start + len as usize).min(self.0.len());
            Ok(Box::new(std::io::Cursor::new(self.0[start..end].to_vec())))
        }
    }

    #[test]
    fn open_range_serves_a_range_source_with_exact_lengths() {
        let data: Vec<u8> = (0..=255u8).collect();
        let entry = MediaEntry {
            content_type: "audio/flac".into(),
            size: data.len() as u64,
            source: MediaSource::Reader(Arc::new(SliceSource(data.clone()))),
        };
        // Whole resource.
        let (mut r, len, code, cr) = open_range(&entry, None).unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!((len, code.0, cr), (256, 200, None));
        assert_eq!(got, data);
        // A middle range, then one that overshoots the end (clamped).
        let (mut r, len, code, cr) = open_range(&entry, Some((10, 19))).unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!((len, code.0), (10, 206));
        assert_eq!(cr.as_deref(), Some("bytes 10-19/256"));
        assert_eq!(got, data[10..=19]);
        let (mut r, len, _, _) = open_range(&entry, Some((250, 999))).unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(len, 6);
        assert_eq!(got, data[250..]);
    }

    #[test]
    fn open_range_streams_file_and_ranges() {
        // A File entry serves the requested range straight from disk with the
        // right status / length / Content-Range (#646 streaming rewrite).
        let bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let path = std::env::temp_dir().join(format!("qbz-cast-test-{}.bin", generate_token()));
        std::fs::write(&path, &bytes).unwrap();
        let entry = MediaEntry {
            content_type: "audio/x-dsf".to_string(),
            size: bytes.len() as u64,
            source: MediaSource::File(path.clone()),
        };

        // Full GET: 200, whole file, streamed through the reader.
        let (mut body, len, status, cr) = open_range(&entry, None).unwrap();
        assert_eq!(status, StatusCode(200));
        assert_eq!(len, bytes.len());
        assert!(cr.is_none());
        let mut served = Vec::new();
        body.read_to_end(&mut served).unwrap();
        assert_eq!(served, bytes);

        // Range GET: 206 + exact slice + Content-Range header value.
        let (mut body, len, status, cr) = open_range(&entry, Some((100, 199))).unwrap();
        assert_eq!(status, StatusCode(206));
        assert_eq!(len, 100);
        assert_eq!(cr.as_deref(), Some("bytes 100-199/4096"));
        let mut served = Vec::new();
        body.read_to_end(&mut served).unwrap();
        assert_eq!(served, bytes[100..=199]);

        std::fs::remove_file(&path).ok();

        // In-memory entries keep the bounded slice copy.
        let entry = MediaEntry {
            content_type: "audio/flac".to_string(),
            size: bytes.len() as u64,
            source: MediaSource::Data(std::sync::Arc::new(bytes.clone())),
        };
        let (mut body, len, status, _) = open_range(&entry, Some((0, 9))).unwrap();
        assert_eq!(status, StatusCode(206));
        assert_eq!(len, 10);
        let mut served = Vec::new();
        body.read_to_end(&mut served).unwrap();
        assert_eq!(served, bytes[..10]);

        // 0-byte entries error out (old read_exact semantics) instead of
        // advertising a lying Content-Length.
        let entry = MediaEntry {
            content_type: "audio/wav".to_string(),
            size: 0,
            source: MediaSource::Data(std::sync::Arc::new(Vec::new())),
        };
        assert!(open_range(&entry, None).is_err());
    }

    #[test]
    fn token_is_random_and_long() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two tokens should differ");
    }
}
