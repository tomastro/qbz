// crates/qbzd/src/api/sse.rs — the `GET /api/events` Server-Sent Events stream
// (CONSOLE ext). A live push feed of CoreEvents so a client (a plasmoid, a bar
// applet, `qbzd watch`) reacts to playback/queue/library changes without
// polling.
//
// Concurrency: the control-plane serve loop is single-threaded, so an open SSE
// stream would starve every other request. `serve()` therefore moves this onto
// its OWN thread (Request is Send); `stream` blocks there until the client
// disconnects (respond() errors) or the bus closes. The rusqlite-free bus is a
// tokio broadcast, drained here with `blocking_recv()` from the plain thread.
//
// Wire format: one SSE frame per emitted event —
//   event: <CoreEvent type>\n
//   data: {"type":"…","data":{…}}\n\n
// The `data` line is the CoreEvent's own `#[serde(tag="type",content="data")]`
// JSON (single-line). A leading `: …` comment primes the stream; a dropped-lag
// notice is sent as a comment rather than silently losing ordering.
use std::io::{Cursor, Read};

use tiny_http::{Header, Request, Response, StatusCode};
use tokio::sync::broadcast;

use qbz_models::CoreEvent;

/// Stream CoreEvents to one client until it disconnects. Runs on a dedicated
/// thread; `req.respond` blocks here, writing chunked as `SseReader` yields.
pub fn stream(req: Request, rx: broadcast::Receiver<CoreEvent>) {
    let headers = vec![
        header("Content-Type", "text/event-stream"),
        header("Cache-Control", "no-cache"),
        header("Connection", "keep-alive"),
        // Defeat proxy/response buffering so frames flush immediately.
        header("X-Accel-Buffering", "no"),
    ];
    let response = Response::new(StatusCode(200), headers, SseReader::new(rx), None, None);
    let _ = req.respond(response);
}

fn header(name: &str, value: &str) -> Header {
    // Both are static ASCII — construction cannot fail.
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

/// A blocking `Read` over the CoreEvent bus: each `read` serves the current
/// frame's bytes, and when a frame is exhausted it blocks for the next event.
/// `Ok(0)` (EOF) only when the bus closes (daemon shutting down).
struct SseReader {
    rx: broadcast::Receiver<CoreEvent>,
    frame: Cursor<Vec<u8>>,
    primed: bool,
}

impl SseReader {
    fn new(rx: broadcast::Receiver<CoreEvent>) -> Self {
        SseReader { rx, frame: Cursor::new(Vec::new()), primed: false }
    }

    /// The next frame to send, blocking for a bus event. `None` = bus closed.
    fn next_frame(&mut self) -> Option<Vec<u8>> {
        if !self.primed {
            self.primed = true;
            return Some(b": qbzd event stream\n\n".to_vec());
        }
        loop {
            match self.rx.blocking_recv() {
                Ok(ev) => {
                    if let Some(frame) = format_event(&ev) {
                        return Some(frame.into_bytes());
                    }
                    // Not an emitted event — keep waiting.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    return Some(format!(": lagged {n} event(s)\n\n").into_bytes());
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

impl Read for SseReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n = self.frame.read(out)?;
            if n > 0 {
                return Ok(n);
            }
            match self.next_frame() {
                Some(bytes) => self.frame = Cursor::new(bytes),
                None => return Ok(0), // bus closed → end the stream
            }
        }
    }
}

/// Render one CoreEvent as an SSE frame, or `None` for events not worth pushing
/// to a UI client (bulky search payloads, internal loading/download/navigation
/// hints, diagnostics). Everything else — playback, queue, volume, auth,
/// favorites, playlists, errors, device changes — is emitted.
fn format_event(ev: &CoreEvent) -> Option<String> {
    if !emit(ev) {
        return None;
    }
    let value = serde_json::to_value(ev).ok()?;
    let typ = value.get("type").and_then(|v| v.as_str()).unwrap_or("event").to_string();
    let data = serde_json::to_string(&value).ok()?;
    Some(format!("event: {typ}\ndata: {data}\n\n"))
}

fn emit(ev: &CoreEvent) -> bool {
    use CoreEvent::*;
    !matches!(
        ev,
        SearchResultsReceived { .. }
            | LoadingStarted { .. }
            | LoadingCompleted { .. }
            | DownloadProgress { .. }
            | DownloadCompleted { .. }
            | NavigateToAlbum { .. }
            | NavigateToArtist { .. }
            | NavigateToPlaylist { .. }
            | AudioDiagnostic { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use qbz_models::PlaybackState;

    #[test]
    fn playback_event_becomes_a_typed_sse_frame() {
        let frame = format_event(&CoreEvent::PlaybackStateChanged {
            state: PlaybackState::Playing,
        })
        .expect("playback event is emitted");
        assert!(frame.starts_with("event: PlaybackStateChanged\n"));
        assert!(frame.contains("data: {"));
        assert!(frame.ends_with("\n\n"));
        // The data line carries the tagged CoreEvent JSON.
        assert!(frame.contains("\"type\":\"PlaybackStateChanged\""));
    }

    #[test]
    fn bulky_and_internal_events_are_not_emitted() {
        assert!(format_event(&CoreEvent::LoadingStarted { operation: "x".into() }).is_none());
        assert!(format_event(&CoreEvent::DownloadCompleted { track_id: 1 }).is_none());
        assert!(format_event(&CoreEvent::NavigateToArtist { artist_id: 1 }).is_none());
    }

    #[test]
    fn volume_and_queue_events_are_emitted() {
        assert!(format_event(&CoreEvent::VolumeChanged { volume: 0.5 }).is_some());
        assert!(format_event(&CoreEvent::ShuffleChanged { enabled: true }).is_some());
    }
}
