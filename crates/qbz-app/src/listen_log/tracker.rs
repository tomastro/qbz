//! The listen tracker: a pure state machine over playback observations.
//!
//! No clock, no I/O — every input carries its own `now`, and every output is
//! a value the caller persists. That is what makes the rules testable and
//! what keeps the two hosts (the Qt poll loop and the `qbzd` event bus)
//! from drifting: both feed the same machine.
//!
//! The one rule that matters (Codex §9, Feishin): `played_ms` accumulates
//! ONLY while playing, from position deltas that are monotonic and at most
//! [`MAX_DELTA_MS`]. A pause adds nothing; a seek (forward or back) is a
//! delta outside that window and adds nothing; a stall that skips reporting
//! for a while adds nothing. Wall-clock and the final position are never
//! consulted — pauses, seeks, repeat and the old scrobble bug all made them
//! lie.

use super::rules::EndReason;

/// A position jump larger than this is a seek or a stall, not audible time.
/// The hosts observe at 1 Hz, so a genuine tick is ~1 s.
pub const MAX_DELTA_MS: u64 = 5_000;

/// Persist the accumulator at least this often (in audible ms) so a crash
/// loses at most this much of the open row.
pub const FLUSH_EVERY_MS: u64 = 10_000;

/// Snapshot of the track a row is opened for. Text is captured here on
/// purpose (Astra): the row must still make sense after the track leaves
/// Qobuz or the Plex server it came from is gone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListenMeta {
    pub source: String,
    pub source_item_id: String,
    pub track_id: Option<i64>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub artwork_key: Option<String>,
    pub duration_ms: u64,
    pub context_kind: String,
    pub context_id: String,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    pub output_backend: Option<String>,
}

/// The open row, as the machine sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenListen {
    /// Store row id, set by the host once the row is inserted.
    pub event_id: i64,
    pub meta: ListenMeta,
    pub started_at: i64,
    pub played_ms: u64,
    /// Last position observed, whatever the play state.
    pub last_position_ms: Option<u64>,
    /// `played_ms` at the last flush the host acknowledged.
    flushed_ms: u64,
}

/// A row to close, with everything the store needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Closed {
    pub event_id: i64,
    pub reason: EndReason,
    pub played_ms: u64,
    pub end_position_ms: Option<u64>,
    pub ended_at: i64,
}

/// A progress checkpoint the host should persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flush {
    pub event_id: i64,
    pub played_ms: u64,
    pub end_position_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ListenTracker {
    open: Option<OpenListen>,
}

impl ListenTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self) -> Option<&OpenListen> {
        self.open.as_ref()
    }

    /// A new track surfaced. Closes the previous row (if any) as a SKIP —
    /// unless the host already closed it as natural — and returns the meta
    /// the host must insert; call [`Self::opened`] with the row id.
    pub fn track_started(&mut self, meta: ListenMeta, now: i64) -> (Option<Closed>, ListenMeta) {
        let closed = self.close_with(EndReason::Skip, now);
        self.open = Some(OpenListen {
            event_id: 0,
            meta: meta.clone(),
            started_at: now,
            played_ms: 0,
            last_position_ms: None,
            flushed_ms: 0,
        });
        (closed, meta)
    }

    /// The host inserted the row for the track from the last
    /// [`Self::track_started`]; from now on progress has somewhere to go.
    pub fn opened(&mut self, event_id: i64) {
        if let Some(open) = self.open.as_mut() {
            open.event_id = event_id;
        }
    }

    /// One playback observation. Returns a flush when the accumulator has
    /// advanced by [`FLUSH_EVERY_MS`] since the last one.
    pub fn tick(&mut self, position_ms: u64, playing: bool) -> Option<Flush> {
        let open = self.open.as_mut()?;
        if let (true, Some(last)) = (playing, open.last_position_ms) {
            if position_ms > last {
                let delta = position_ms - last;
                if delta <= MAX_DELTA_MS {
                    open.played_ms += delta;
                }
            }
        }
        open.last_position_ms = Some(position_ms);
        if open.event_id != 0 && open.played_ms.saturating_sub(open.flushed_ms) >= FLUSH_EVERY_MS {
            open.flushed_ms = open.played_ms;
            return Some(Flush {
                event_id: open.event_id,
                played_ms: open.played_ms,
                end_position_ms: open.last_position_ms,
            });
        }
        None
    }

    /// The host's own natural-end predicate fired.
    pub fn ended_naturally(&mut self, now: i64) -> Option<Closed> {
        self.close_with(EndReason::Natural, now)
    }

    /// Playback stopped with nothing following. A host without an explicit
    /// natural-end edge (qbzd) passes `infer_natural = true`: a stop whose
    /// last position sat within 2 s of the duration is the track ending.
    pub fn stopped(&mut self, now: i64, infer_natural: bool) -> Option<Closed> {
        let reason = if infer_natural && self.near_end() {
            EndReason::Natural
        } else {
            EndReason::Stop
        };
        self.close_with(reason, now)
    }

    pub fn errored(&mut self, now: i64) -> Option<Closed> {
        self.close_with(EndReason::Error, now)
    }

    pub fn shutdown(&mut self, now: i64) -> Option<Closed> {
        self.close_with(EndReason::Shutdown, now)
    }

    /// Playback authority moved to another renderer. The owner row is closed
    /// explicitly and no delegated playback is folded into it. Calling this
    /// more than once is harmless: after the first close there is no open row.
    pub fn handoff(&mut self, now: i64) -> Option<Closed> {
        self.close_with(EndReason::Handoff, now)
    }

    /// For hosts that only see "a different track started": was the previous
    /// one within 2 s of its end? Mirrors the desktop's
    /// `seen_position + 2 >= duration` (playback_driver.rs:226-264).
    pub fn near_end(&self) -> bool {
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        let duration = open.meta.duration_ms;
        match open.last_position_ms {
            Some(pos) if duration > 0 => pos + 2_000 >= duration,
            _ => false,
        }
    }

    /// Close with an inferred reason: natural when near the end, else skip.
    /// The qbzd bus has no TrackEnded (nobody emits it), so this is its
    /// track-edge close.
    pub fn track_started_inferring_end(
        &mut self,
        meta: ListenMeta,
        now: i64,
    ) -> (Option<Closed>, ListenMeta) {
        let reason = if self.near_end() {
            EndReason::Natural
        } else {
            EndReason::Skip
        };
        let closed = self.close_with(reason, now);
        self.open = Some(OpenListen {
            event_id: 0,
            meta: meta.clone(),
            started_at: now,
            played_ms: 0,
            last_position_ms: None,
            flushed_ms: 0,
        });
        (closed, meta)
    }

    fn close_with(&mut self, reason: EndReason, now: i64) -> Option<Closed> {
        let open = self.open.take()?;
        // A row the host never managed to insert has nothing to close.
        if open.event_id == 0 {
            return None;
        }
        Some(Closed {
            event_id: open.event_id,
            reason,
            played_ms: open.played_ms,
            end_position_ms: open.last_position_ms,
            ended_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(duration_ms: u64) -> ListenMeta {
        ListenMeta {
            source: "qobuz".into(),
            source_item_id: "1".into(),
            title: "T".into(),
            artist: "A".into(),
            duration_ms,
            ..Default::default()
        }
    }

    fn started(t: &mut ListenTracker, id: i64) {
        let (_, _) = t.track_started(meta(200_000), 1_000);
        t.opened(id);
    }

    #[test]
    fn accumulates_only_monotonic_small_deltas_while_playing() {
        let mut t = ListenTracker::new();
        started(&mut t, 7);
        assert_eq!(t.tick(0, true), None); // first observation: no delta
        t.tick(1_000, true);
        t.tick(2_000, true);
        assert_eq!(t.open().unwrap().played_ms, 2_000);
        // Seek forward 60 s: ignored, but the position is remembered.
        t.tick(62_000, true);
        assert_eq!(t.open().unwrap().played_ms, 2_000);
        t.tick(63_000, true);
        assert_eq!(t.open().unwrap().played_ms, 3_000);
        // Seek back: negative delta ignored.
        t.tick(10_000, true);
        assert_eq!(t.open().unwrap().played_ms, 3_000);
        t.tick(11_000, true);
        assert_eq!(t.open().unwrap().played_ms, 4_000);
        // Exactly MAX_DELTA counts; one more does not.
        t.tick(16_000, true);
        assert_eq!(t.open().unwrap().played_ms, 9_000);
        t.tick(21_001, true);
        assert_eq!(t.open().unwrap().played_ms, 9_000);
    }

    #[test]
    fn paused_ticks_add_nothing_and_survive_the_pause() {
        let mut t = ListenTracker::new();
        started(&mut t, 7);
        t.tick(0, true);
        t.tick(1_000, true);
        // Two minutes paused at the same position.
        for _ in 0..120 {
            t.tick(1_000, false);
        }
        assert_eq!(t.open().unwrap().played_ms, 1_000);
        // Resume: the next delta is 1 s again.
        t.tick(2_000, true);
        assert_eq!(t.open().unwrap().played_ms, 2_000);
        // A paused tick whose position moved (a seek while paused) counts nothing.
        t.tick(50_000, false);
        t.tick(51_000, true);
        assert_eq!(t.open().unwrap().played_ms, 3_000);
    }

    #[test]
    fn flushes_every_ten_audible_seconds() {
        let mut t = ListenTracker::new();
        started(&mut t, 7);
        t.tick(0, true);
        let mut flushes = Vec::new();
        for s in 1..=25u64 {
            if let Some(f) = t.tick(s * 1_000, true) {
                flushes.push(f);
            }
        }
        assert_eq!(
            flushes,
            vec![
                Flush {
                    event_id: 7,
                    played_ms: 10_000,
                    end_position_ms: Some(10_000)
                },
                Flush {
                    event_id: 7,
                    played_ms: 20_000,
                    end_position_ms: Some(20_000)
                },
            ]
        );
    }

    #[test]
    fn next_track_closes_the_previous_as_skip_with_its_accumulator() {
        let mut t = ListenTracker::new();
        started(&mut t, 7);
        t.tick(0, true);
        for s in 1..=10u64 {
            t.tick(s * 1_000, true);
        }
        let (closed, next) = t.track_started(meta(100_000), 5_000);
        assert_eq!(
            closed,
            Some(Closed {
                event_id: 7,
                reason: EndReason::Skip,
                played_ms: 10_000,
                end_position_ms: Some(10_000),
                ended_at: 5_000,
            })
        );
        assert_eq!(next.duration_ms, 100_000);
        assert_eq!(t.open().unwrap().event_id, 0);
        t.opened(8);
        assert_eq!(t.open().unwrap().event_id, 8);
    }

    #[test]
    fn natural_end_then_next_track_opens_without_a_second_close() {
        let mut t = ListenTracker::new();
        started(&mut t, 7);
        t.tick(0, true);
        t.tick(199_000, true);
        let closed = t.ended_naturally(9_000).unwrap();
        assert_eq!(closed.reason, EndReason::Natural);
        let (closed_again, _) = t.track_started(meta(100_000), 9_001);
        assert_eq!(closed_again, None);
    }

    #[test]
    fn stop_shutdown_error_handoff_reasons() {
        let mut t = ListenTracker::new();
        started(&mut t, 1);
        assert_eq!(t.stopped(2, false).unwrap().reason, EndReason::Stop);
        started(&mut t, 2);
        assert_eq!(t.shutdown(3).unwrap().reason, EndReason::Shutdown);
        started(&mut t, 3);
        assert_eq!(t.errored(4).unwrap().reason, EndReason::Error);
        started(&mut t, 4);
        assert_eq!(t.handoff(5).unwrap().reason, EndReason::Handoff);
        assert_eq!(t.handoff(6), None, "handoff is idempotent");
        assert_eq!(t.shutdown(7), None);
    }

    #[test]
    fn inferred_end_uses_the_two_second_window() {
        let mut t = ListenTracker::new();
        started(&mut t, 1);
        t.tick(0, true);
        t.tick(198_000, true);
        assert!(t.near_end());
        let (closed, _) = t.track_started_inferring_end(meta(50_000), 9);
        assert_eq!(closed.unwrap().reason, EndReason::Natural);
        t.opened(2);
        t.tick(0, true);
        t.tick(10_000, true);
        assert!(!t.near_end());
        assert_eq!(t.stopped(10, true).unwrap().reason, EndReason::Stop);
        started(&mut t, 3);
        t.tick(198_500, true);
        assert_eq!(t.stopped(11, true).unwrap().reason, EndReason::Natural);
    }

    #[test]
    fn a_row_that_was_never_inserted_closes_to_nothing() {
        let mut t = ListenTracker::new();
        let _ = t.track_started(meta(1), 1);
        // no `opened` call: the insert failed or paused
        t.tick(0, true);
        t.tick(1_000, true);
        assert_eq!(t.tick(11_000, true), None);
        assert_eq!(t.ended_naturally(2), None);
        assert!(t.open().is_none());
    }
}
