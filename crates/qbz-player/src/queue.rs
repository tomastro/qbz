//! Queue management module
//!
//! Handles playback queue with:
//! - Queue manipulation (add, remove, reorder, clear)
//! - Current track tracking
//! - Shuffle mode
//! - Repeat modes (off, all, one)
//! - Play history for going back

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use qbz_models::{QueueState, QueueTrack, RepeatMode};

#[derive(Debug, PartialEq, Eq)]
enum QueueMoveDirection {
    Up,
    Down,
}

const MAX_HISTORY_LEN: usize = 50;

/// Internal queue state - all in one struct to avoid deadlocks
#[derive(Clone)]
struct InternalState {
    /// All tracks in the queue (original order)
    tracks: Vec<QueueTrack>,
    /// Current playback index
    current_index: Option<usize>,
    /// Shuffle mode enabled
    shuffle: bool,
    /// Shuffled indices (when shuffle is on)
    shuffle_order: Vec<usize>,
    /// Position in shuffle order
    shuffle_position: usize,
    /// Repeat mode
    repeat: RepeatMode,
    /// History of played track indices (for going back)
    history: VecDeque<usize>,
    /// Track ID to stop after (optional)
    stop_after_track_id: Option<u64>,
    /// Manual-block size (#442): how many entries right after `current_index`
    /// were added by hand ("Play next" / "Play later"). The block always
    /// plays before the source (album/playlist) resumes; "Add to queue" does
    /// NOT extend it (it appends to the absolute end, untouched).
    manual_next_count: usize,
}

/// An exact, in-process snapshot of queue playback authority.
///
/// The payload is deliberately opaque: callers can preserve and later restore
/// the queue, but cannot manufacture a partial or internally inconsistent
/// state. It is not a persistence format and is intended for transactional
/// authority handoffs such as QConnect delegation.
#[must_use = "an authority snapshot must be retained until it is restored or deliberately discarded"]
#[derive(Clone)]
pub struct QueueAuthoritySnapshot(InternalState);

/// Queue manager for handling playback queue
pub struct QueueManager {
    state: Mutex<InternalState>,
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueManager {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(InternalState {
                tracks: Vec::new(),
                current_index: None,
                shuffle: false,
                shuffle_order: Vec::new(),
                shuffle_position: 0,
                repeat: RepeatMode::Off,
                history: VecDeque::with_capacity(50),
                stop_after_track_id: None,
                manual_next_count: 0,
            }),
        }
    }

    /// The track at the current playback index, if any.
    pub fn current(&self) -> Option<QueueTrack> {
        let state = self.state.lock().unwrap();
        state
            .current_index
            .and_then(|idx| state.tracks.get(idx).cloned())
    }

    /// Patch the cached quality (bit depth + sample rate) of every queued track
    /// whose Plex `rating_key` matches one of `updates`. Used by the Plex
    /// quality-hydration path so a track that gets hydrated while it is already
    /// enqueued/playing has its frozen quality snapshot upgraded in place. Plex
    /// rows carry their `rating_key` in `source_item_id_hint`. `sample_rate` is
    /// in kHz to match the enqueue-time snapshot (`local_queue_track`). Returns
    /// true if the CURRENT track was among those patched (the caller then
    /// re-pushes the now-playing stamp).
    pub fn patch_plex_quality(&self, updates: &[(String, Option<u32>, Option<f64>)]) -> bool {
        if updates.is_empty() {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        let current_idx = state.current_index;
        let mut current_patched = false;
        for (idx, track) in state.tracks.iter_mut().enumerate() {
            if track.source.as_deref() != Some("plex") {
                continue;
            }
            let Some(key) = track.source_item_id_hint.as_deref() else {
                continue;
            };
            if let Some((_, bit_depth, sample_rate)) =
                updates.iter().find(|(k, _, _)| k == key)
            {
                if let Some(bd) = *bit_depth {
                    track.bit_depth = Some(bd);
                    track.hires = bd > 16;
                }
                if let Some(sr) = *sample_rate {
                    track.sample_rate = Some(sr);
                }
                if current_idx == Some(idx) {
                    current_patched = true;
                }
            }
        }
        current_patched
    }

    /// Attach an artwork url to every queued track whose id is in `ids`.
    ///
    /// The sibling of [`Self::patch_plex_quality`], for the other thing a row
    /// can learn AFTER it was enqueued: a cover. A disc carries none — it is
    /// fetched, and the fetch outlives the click that started playback — so a
    /// CD queued at second two is still holding `artwork_url: None` when the
    /// cover lands at second ten. Every consumer downstream reads the QUEUE
    /// row (the now-playing bar, the miniplayer, MPRIS through
    /// `art_url_for`), so patching the session store alone leaves all of them
    /// blank.
    ///
    /// Rows that already have art are left alone: this fills a gap, it does
    /// not overwrite a cover somebody else resolved.
    ///
    /// Returns `(any patched, the CURRENT track was patched)`. The two are
    /// separate answers because they drive different work: any patch makes the
    /// queue panel's thumbnails stale, while only the current one needs the
    /// now-playing stamp re-pushed.
    pub fn patch_artwork(&self, ids: &[u64], url: &str) -> (bool, bool) {
        if ids.is_empty() || url.is_empty() {
            return (false, false);
        }
        let mut state = self.state.lock().unwrap();
        let current_idx = state.current_index;
        let mut any = false;
        let mut current_patched = false;
        for (idx, track) in state.tracks.iter_mut().enumerate() {
            if track.artwork_url.as_deref().is_some_and(|u| !u.is_empty()) {
                continue;
            }
            if !ids.contains(&track.id) {
                continue;
            }
            track.artwork_url = Some(url.to_string());
            any = true;
            if current_idx == Some(idx) {
                current_patched = true;
            }
        }
        (any, current_patched)
    }

    /// Add a track to the end of the queue
    pub fn add_track(&self, track: QueueTrack) {
        let mut state = self.state.lock().unwrap();
        state.tracks.push(track);

        if state.shuffle {
            let new_idx = state.tracks.len() - 1;
            state.shuffle_order.push(new_idx);
        }
    }

    /// Add multiple tracks to the queue
    pub fn add_tracks(&self, new_tracks: Vec<QueueTrack>) {
        let mut state = self.state.lock().unwrap();
        let start_idx = state.tracks.len();
        state.tracks.extend(new_tracks);

        if state.shuffle {
            for i in start_idx..state.tracks.len() {
                state.shuffle_order.push(i);
            }
        }
    }

    /// Add a track to play next (after current index if set)
    pub fn add_track_next(&self, track: QueueTrack) {
        let mut state = self.state.lock().unwrap();
        let insert_index = state.current_index.map(|idx| idx + 1).unwrap_or(0);

        if insert_index >= state.tracks.len() {
            state.tracks.push(track);
        } else {
            state.tracks.insert(insert_index, track);
        }
        // The track joins the manual block (#442).
        state.manual_next_count += 1;

        if state.shuffle {
            for idx in state.shuffle_order.iter_mut() {
                if *idx >= insert_index {
                    *idx += 1;
                }
            }

            let new_idx = insert_index;
            let next_pos = if state.current_index.is_some() {
                state.shuffle_position + 1
            } else {
                state.shuffle_order.len()
            };

            if next_pos >= state.shuffle_order.len() {
                state.shuffle_order.push(new_idx);
            } else {
                state.shuffle_order.insert(next_pos, new_idx);
            }
        }
    }

    /// Add a track at the END of the manual block (#442 "Play later"): after
    /// every play-next / play-later already queued, before the source
    /// (album/playlist) resumes. "Add to queue" stays untouched at the
    /// absolute end. With shuffle on, degrades to play-next — the block
    /// concept is meaningless once the order is reshuffled.
    pub fn add_track_later(&self, track: QueueTrack) {
        let mut state = self.state.lock().unwrap();
        let insert_index = if state.shuffle {
            state.current_index.map(|idx| idx + 1).unwrap_or(0)
        } else {
            state
                .current_index
                .map(|idx| idx + 1 + state.manual_next_count)
                .unwrap_or(state.manual_next_count)
        };
        let insert_index = insert_index.min(state.tracks.len());
        state.tracks.insert(insert_index, track);
        state.manual_next_count += 1;

        if state.shuffle {
            for idx in state.shuffle_order.iter_mut() {
                if *idx >= insert_index {
                    *idx += 1;
                }
            }
            let new_idx = insert_index;
            let next_pos = if state.current_index.is_some() {
                state.shuffle_position + 1
            } else {
                state.shuffle_order.len()
            };
            if next_pos >= state.shuffle_order.len() {
                state.shuffle_order.push(new_idx);
            } else {
                state.shuffle_order.insert(next_pos, new_idx);
            }
        }
    }

    /// Set the entire queue (replaces existing)
    pub fn set_queue(&self, new_tracks: Vec<QueueTrack>, start_index: Option<usize>) {
        let mut state = self.state.lock().unwrap();
        state.stop_after_track_id = None;
        // A full replacement is a new source: the manual block dissolves (#442).
        state.manual_next_count = 0;
        // Remap history by track id BEFORE replacing tracks so that legitimate
        // plays survive queue version bumps / reorders. Entries whose track is
        // no longer present are dropped. See bug #316.
        Self::remap_history_by_track_id_internal(&mut state, &new_tracks);
        state.tracks = new_tracks;
        state.current_index = start_index;
        if let Some(current_index) = state.current_index {
            state.history.retain(|&index| index != current_index);
        }

        // Regenerate shuffle order
        Self::regenerate_shuffle_order_internal(&mut state);

        // CRITICAL FIX: When shuffle is enabled and we have a start_index,
        // ensure the start_index track is at the BEGINNING of shuffle order
        if state.shuffle {
            if let Some(start_idx) = start_index {
                if start_idx < state.tracks.len() {
                    if let Some(pos) = state.shuffle_order.iter().position(|&x| x == start_idx) {
                        state.shuffle_order.swap(0, pos);
                        state.shuffle_position = 0;

                        log::info!(
                            "Queue: Adjusted shuffle order to start with track index {} (was at position {})",
                            start_idx,
                            pos
                        );
                    }
                }
            }
        }
    }

    /// Replace the queue and playback order in a single atomic update.
    /// This avoids emitting an intermediate locally reshuffled state before an
    /// authoritative remote shuffle order has been applied.
    pub fn set_queue_with_order(
        &self,
        new_tracks: Vec<QueueTrack>,
        start_index: Option<usize>,
        shuffle_enabled: bool,
        shuffle_order: Option<Vec<usize>>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.stop_after_track_id = None;
        // Remap history by track id BEFORE replacing tracks so that legitimate
        // plays survive queue version bumps / reorders. Entries whose track is
        // no longer present are dropped. See bug #316.
        Self::remap_history_by_track_id_internal(&mut state, &new_tracks);
        state.tracks = new_tracks;
        state.current_index = start_index;
        if let Some(current_index) = state.current_index {
            state.history.retain(|&index| index != current_index);
        }
        state.shuffle = shuffle_enabled;

        if !shuffle_enabled {
            state.shuffle_order.clear();
            state.shuffle_position = 0;
            return;
        }

        if let Some(order) =
            shuffle_order.filter(|order| Self::is_valid_shuffle_order(order, state.tracks.len()))
        {
            state.shuffle_order = order;
            if let Some(curr_idx) = state.current_index {
                if let Some(pos) = state.shuffle_order.iter().position(|&idx| idx == curr_idx) {
                    state.shuffle_position = pos;
                } else {
                    state.shuffle_position = 0;
                }
            } else {
                state.shuffle_position = 0;
            }
            return;
        }

        Self::set_identity_shuffle_order_internal(&mut state);
    }

    /// Clear the queue.
    ///
    /// When `keep_current` is true (default / historical behavior), the track
    /// at `current_index` is preserved as the sole remaining entry so the
    /// "now playing" slot doesn't go dark mid-song. Callers that know nothing
    /// is playing (or want to fully reset) can pass `false` to wipe everything,
    /// including the current track.
    pub fn clear(&self, keep_current: bool) {
        let mut state = self.state.lock().unwrap();
        state.stop_after_track_id = None;
        state.manual_next_count = 0;

        if keep_current {
            // Keep the track at `current_index`, not always `tracks[0]`.
            // `truncate(1)` was wrong mid-queue: clear while playing track N
            // would leave the first row as now-playing while audio kept N.
            if let Some(idx) = state.current_index {
                if idx < state.tracks.len() {
                    let kept = state.tracks[idx].clone();
                    // History stores indices into `tracks`. Remap by track id
                    // so entries for removed rows drop and any entry that still
                    // refers to the kept track points at index 0.
                    Self::remap_history_by_track_id_internal(
                        &mut state,
                        std::slice::from_ref(&kept),
                    );
                    state.tracks = vec![kept];
                    state.current_index = Some(0);
                } else {
                    state.tracks.clear();
                    state.current_index = None;
                    state.history.clear();
                }
            } else {
                state.tracks.clear();
                state.current_index = None;
                state.history.clear();
            }
        } else {
            state.tracks.clear();
            state.current_index = None;
            // Indices into an empty list are meaningless.
            state.history.clear();
        }

        state.shuffle_order.clear();
        state.shuffle_position = 0;
        // Playback history is remapped (or cleared) above. "Clear queue" only
        // affects current/upcoming queue rows, not an intentional history wipe
        // when the kept track still resolves — but removed tracks cannot stay
        // in index-based history.
    }

    /// Remove a track by index
    pub fn remove_track(&self, index: usize) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();
        if index >= state.tracks.len() {
            return None;
        }

        let removed = state.tracks.remove(index);

        // Removing an entry inside the manual block shrinks it (#442).
        if let Some(curr_idx) = state.current_index {
            if index > curr_idx && index <= curr_idx + state.manual_next_count {
                state.manual_next_count = state.manual_next_count.saturating_sub(1);
            }
        }

        // Invalidate marker if the removed track matches
        if state.stop_after_track_id == Some(removed.id) {
            state.stop_after_track_id = None;
        }

        // Adjust current index if needed
        if let Some(curr_idx) = state.current_index {
            if index < curr_idx {
                state.current_index = Some(curr_idx - 1);
            } else if index == curr_idx {
                if curr_idx >= state.tracks.len() {
                    state.current_index = if state.tracks.is_empty() {
                        None
                    } else {
                        Some(state.tracks.len() - 1)
                    };
                }
            }
        }

        // Keep history indices aligned with current track list after removal.
        state.history.retain(|&hist_idx| hist_idx != index);
        for hist_idx in state.history.iter_mut() {
            if *hist_idx > index {
                *hist_idx -= 1;
            }
        }

        if state.shuffle {
            Self::remove_index_from_shuffle_internal(&mut state, index);
        }
        Some(removed)
    }

    /// Remove a track by its position in the upcoming list
    pub fn remove_upcoming_track(&self, upcoming_index: usize) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();

        let actual_index = if state.shuffle {
            let shuffle_pos = state.shuffle_position + 1 + upcoming_index;
            if shuffle_pos >= state.shuffle_order.len() {
                return None;
            }
            state.shuffle_order[shuffle_pos]
        } else {
            match state.current_index {
                Some(curr_idx) => curr_idx + 1 + upcoming_index,
                None => upcoming_index,
            }
        };

        if actual_index >= state.tracks.len() {
            return None;
        }

        log::info!(
            "remove_upcoming_track: upcoming_index={} -> actual_index={}",
            upcoming_index,
            actual_index
        );

        let removed = state.tracks.remove(actual_index);

        // Invalidate marker if the removed track matches
        if state.stop_after_track_id == Some(removed.id) {
            state.stop_after_track_id = None;
        }

        if let Some(curr_idx) = state.current_index {
            if actual_index < curr_idx {
                state.current_index = Some(curr_idx - 1);
            } else if actual_index == curr_idx {
                if curr_idx >= state.tracks.len() {
                    state.current_index = if state.tracks.is_empty() {
                        None
                    } else {
                        Some(state.tracks.len() - 1)
                    };
                }
            }
        }

        // Keep history indices aligned with current track list after removal.
        state.history.retain(|&hist_idx| hist_idx != actual_index);
        for hist_idx in state.history.iter_mut() {
            if *hist_idx > actual_index {
                *hist_idx -= 1;
            }
        }

        if state.shuffle {
            Self::remove_index_from_shuffle_internal(&mut state, actual_index);
        }
        Some(removed)
    }

    /// Number of upcoming tracks (those after the current one) in the current
    /// play order. Mirrors `get_state_full`'s upcoming computation: shuffle-aware
    /// when shuffle is on, otherwise the tail of `tracks` after `current_index`.
    fn upcoming_len(state: &InternalState) -> usize {
        match state.current_index {
            Some(curr) => {
                if state.shuffle {
                    state
                        .shuffle_order
                        .len()
                        .saturating_sub(state.shuffle_position + 1)
                } else {
                    state.tracks.len().saturating_sub(curr + 1)
                }
            }
            None => state.tracks.len(),
        }
    }

    /// Remove every upcoming track positioned AFTER `upcoming_index` in the
    /// current play order; the track at `upcoming_index` is kept. Works in
    /// UPCOMING space (not absolute `tracks` indices), so it stays correct under
    /// shuffle by reusing `remove_upcoming_track`, which resolves upcoming
    /// positions through `shuffle_order`. Peels positions off the tail inward so
    /// the surviving positions never shift under it. Returns the count removed.
    ///
    /// This is the wired "Remove all after" queue action. (`remove_after`, below,
    /// truncates by absolute `tracks` index and is NOT play-order-aware under
    /// shuffle — it is kept only for its existing unit coverage.)
    pub fn remove_upcoming_after(&self, upcoming_index: usize) -> usize {
        let mut upcoming_len = {
            let state = self.state.lock().unwrap();
            Self::upcoming_len(&state)
        };
        if upcoming_index + 1 >= upcoming_len {
            return 0;
        }
        let mut removed = 0usize;
        while upcoming_len > upcoming_index + 1 {
            if self.remove_upcoming_track(upcoming_len - 1).is_none() {
                break;
            }
            removed += 1;
            upcoming_len -= 1;
        }
        removed
    }

    /// Remove all tracks at indices greater than `index`. The track at
    /// `index` is preserved. Returns the number of tracks removed.
    /// If the marker referenced a track in the removed range, the marker
    /// is cleared. No-op (returns 0) if `index` is the last position or
    /// out of bounds.
    pub fn remove_after(&self, index: usize) -> usize {
        let mut state = self.state.lock().unwrap();

        if index + 1 >= state.tracks.len() {
            return 0;
        }

        let cutoff = index + 1;
        let removed_ids: Vec<u64> = state.tracks[cutoff..].iter().map(|t| t.id).collect();
        let removed_count = removed_ids.len();

        // Drop the tail of `tracks`.
        state.tracks.truncate(cutoff);

        // If shuffle is active, also drop indices >= cutoff from shuffle_order
        // (preserve relative order of surviving indices).
        if state.shuffle {
            state.shuffle_order.retain(|&i| i < cutoff);
            // shuffle_position remains valid since we only dropped tracks AFTER
            // the current playing one (precondition: index >= current_index in
            // the typical UI flow; defensive clamp below handles edge cases).
            if state.shuffle_position >= state.shuffle_order.len() {
                state.shuffle_position = state.shuffle_order.len().saturating_sub(1);
            }
        }

        // Drop history entries pointing past the cutoff.
        state.history.retain(|&i| i < cutoff);

        // Invalidate marker if it pointed into the removed range.
        if let Some(marker_id) = state.stop_after_track_id {
            if removed_ids.contains(&marker_id) {
                state.stop_after_track_id = None;
            }
        }

        removed_count
    }

    /// Move a track from one position to another
    pub fn move_track(&self, from_index: usize, to_index: usize) -> bool {
        let mut state = self.state.lock().unwrap();

        if state.shuffle {
            // In shuffle mode, DnD indices come from the visible upcoming list,
            // so they must be applied to shuffle_order positions (not absolute
            // track indices in state.tracks).
            let base_pos = state
                .current_index
                .map(|_| state.shuffle_position + 1)
                .unwrap_or(0);
            let from_pos = base_pos + from_index;
            let to_pos = base_pos + to_index;

            if from_pos >= state.shuffle_order.len() || to_pos >= state.shuffle_order.len() {
                return false;
            }

            if from_pos == to_pos {
                return true;
            }

            let moved = state.shuffle_order.remove(from_pos);
            state.shuffle_order.insert(to_pos, moved);

            if let Some(curr_idx) = state.current_index {
                if let Some(pos) = state.shuffle_order.iter().position(|&x| x == curr_idx) {
                    state.shuffle_position = pos;
                }
            } else {
                state.shuffle_position = 0;
            }

            return true;
        }

        let direction: QueueMoveDirection = if from_index > to_index {
            QueueMoveDirection::Up
        } else {
            QueueMoveDirection::Down
        };

        let mut from_idx = from_index;
        let mut to_idx = to_index;

        if let Some(curr_idx) = state.current_index {
            from_idx = from_idx + curr_idx + 1;
            to_idx = to_idx + curr_idx + 1;
        }

        if direction == QueueMoveDirection::Down {
            to_idx = to_idx - 1;
        }

        log::info!(
            "Queue: move_track - {:?} from {} to {} (internal indices:{} -> {}). Tracks in queue: {}",
            direction,
            from_index,
            to_index,
            from_idx,
            to_idx,
            state.tracks.len()
        );

        if from_idx == to_idx {
            return true;
        }

        if from_idx >= state.tracks.len() || to_idx >= state.tracks.len() {
            return false;
        }

        let track = state.tracks.remove(from_idx);
        state.tracks.insert(to_idx, track);

        if let Some(curr_idx) = state.current_index {
            if from_idx == curr_idx {
                state.current_index = Some(to_idx);
            } else if from_idx < curr_idx && to_idx >= curr_idx {
                state.current_index = Some(curr_idx - 1);
            } else if from_idx > curr_idx && to_idx <= curr_idx {
                state.current_index = Some(curr_idx + 1);
            }
        }

        // Keep history aligned after reorder.
        for hist_idx in state.history.iter_mut() {
            *hist_idx = Self::remap_index_after_move(*hist_idx, from_idx, to_idx);
        }

        true
    }

    /// Reorder one playback-history occurrence. `history_index` uses the
    /// public most-recent-first coordinate while `to_slot` is an insertion
    /// gap in the chronological (oldest-first) Queue View projection.
    ///
    /// History stores canonical queue indices, not track ids. Consequently two
    /// independently enqueued copies of the same song remain independently
    /// movable here.
    pub fn move_history_entry(
        &self,
        history_index: usize,
        expected_id: u64,
        to_slot: usize,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(source_position) = state
            .history
            .len()
            .checked_sub(history_index.saturating_add(1))
        else {
            return false;
        };
        let Some(&canonical_index) = state.history.get(source_position) else {
            return false;
        };
        if state.tracks.get(canonical_index).map(|track| track.id) != Some(expected_id) {
            return false;
        }

        let slot = to_slot.min(state.history.len());
        if slot == source_position || slot == source_position + 1 {
            return false;
        }
        let Some(entry) = state.history.remove(source_position) else {
            return false;
        };
        let insertion = if slot > source_position {
            slot - 1
        } else {
            slot
        };
        let insertion = insertion.min(state.history.len());
        state.history.insert(insertion, entry);
        true
    }

    /// Remove one Queue View history occurrence after an authoritative remote
    /// renderer accepted it as a new upcoming item. The queue row itself stays
    /// in place until the remote QueueUpdated echo materializes its canonical
    /// order locally.
    pub fn remove_history_entry(&self, history_index: usize, expected_id: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(source_position) = state
            .history
            .len()
            .checked_sub(history_index.saturating_add(1))
        else {
            return false;
        };
        let Some(&canonical_index) = state.history.get(source_position) else {
            return false;
        };
        if state.tracks.get(canonical_index).map(|track| track.id) != Some(expected_id) {
            return false;
        }

        let before = state.history.len();
        // Historical sessions could contain the same canonical occurrence
        // more than once. Removing all of those stale references establishes
        // the same-entry/latest-play invariant immediately.
        state.history.retain(|&index| index != canonical_index);
        state.history.len() != before
    }

    /// Move a played queue occurrence back into an Upcoming insertion slot.
    /// This is deliberately a MOVE of the canonical occurrence, never an
    /// `add_track` clone. Distinct queue occurrences may share a track id and
    /// remain distinct; replaying one occurrence merely relocates that one.
    pub fn requeue_history_entry(
        &self,
        history_index: usize,
        expected_id: u64,
        to_slot: usize,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(source_history_position) = state
            .history
            .len()
            .checked_sub(history_index.saturating_add(1))
        else {
            return false;
        };
        let Some(&source_index) = state.history.get(source_history_position) else {
            return false;
        };
        if state.tracks.get(source_index).map(|track| track.id) != Some(expected_id)
            || state.current_index == Some(source_index)
        {
            return false;
        }

        if state.shuffle {
            let Some(source_order_position) = state
                .shuffle_order
                .iter()
                .position(|&index| index == source_index)
            else {
                return false;
            };
            let current_order_position = match state.current_index {
                Some(current_index) => {
                    let Some(position) = state
                        .shuffle_order
                        .iter()
                        .position(|&index| index == current_index)
                    else {
                        return false;
                    };
                    Some(position)
                }
                None => None,
            };
            let source_upcoming_position = match current_order_position {
                Some(current_position) if source_order_position > current_position => {
                    Some(source_order_position - current_position - 1)
                }
                Some(_) => None,
                None => Some(source_order_position),
            };
            let current_after_removal = current_order_position.map(|current_position| {
                if source_order_position < current_position {
                    current_position - 1
                } else {
                    current_position
                }
            });

            state.history.retain(|&index| index != source_index);
            state.shuffle_order.remove(source_order_position);
            let base_position = current_after_removal.map(|position| position + 1).unwrap_or(0);
            let insertion_slot = match source_upcoming_position {
                Some(position) if to_slot > position => to_slot - 1,
                _ => to_slot,
            }
            .min(state.shuffle_order.len().saturating_sub(base_position));
            state
                .shuffle_order
                .insert(base_position + insertion_slot, source_index);
            state.shuffle_position = current_after_removal.unwrap_or(0);
            return true;
        }

        let current_before = state.current_index;
        let source_upcoming_position = match current_before {
            Some(current_index) if source_index > current_index => {
                Some(source_index - current_index - 1)
            }
            Some(_) => None,
            None => Some(source_index),
        };
        let current_after_removal = current_before.map(|current_index| {
            if source_index < current_index {
                current_index - 1
            } else {
                current_index
            }
        });
        let base_index = current_after_removal.map(|index| index + 1).unwrap_or(0);
        let insertion_slot = match source_upcoming_position {
            Some(position) if to_slot > position => to_slot - 1,
            _ => to_slot,
        }
        .min(
            state
                .tracks
                .len()
                .saturating_sub(1)
                .saturating_sub(base_index),
        );
        let destination_index = base_index + insertion_slot;

        state.history.retain(|&index| index != source_index);
        let track = state.tracks.remove(source_index);
        state.tracks.insert(destination_index, track);
        state.current_index = current_before
            .map(|index| Self::remap_index_after_move(index, source_index, destination_index));
        for history_index in state.history.iter_mut() {
            *history_index =
                Self::remap_index_after_move(*history_index, source_index, destination_index);
        }
        true
    }

    /// Get current track
    pub fn current_track(&self) -> Option<QueueTrack> {
        let state = self.state.lock().unwrap();
        state
            .current_index
            .and_then(|idx| state.tracks.get(idx).cloned())
    }

    /// Get next track without advancing
    pub fn peek_next(&self) -> Option<QueueTrack> {
        let state = self.state.lock().unwrap();
        if state.tracks.is_empty() {
            return None;
        }

        if state.repeat == RepeatMode::One {
            return state
                .current_index
                .and_then(|idx| state.tracks.get(idx).cloned());
        }

        let next_idx = if state.shuffle {
            let next_pos = state.shuffle_position + 1;
            if next_pos < state.shuffle_order.len() {
                Some(state.shuffle_order[next_pos])
            } else if state.repeat == RepeatMode::All {
                state.shuffle_order.first().copied()
            } else {
                None
            }
        } else {
            let curr_idx = state.current_index.unwrap_or(0);
            let next_idx = curr_idx + 1;
            if next_idx < state.tracks.len() {
                Some(next_idx)
            } else if state.repeat == RepeatMode::All {
                Some(0)
            } else {
                None
            }
        };

        next_idx.and_then(|idx| state.tracks.get(idx).cloned())
    }

    /// Get multiple upcoming tracks without advancing
    pub fn peek_upcoming(&self, count: usize) -> Vec<QueueTrack> {
        let state = self.state.lock().unwrap();
        if state.tracks.is_empty() || count == 0 {
            return Vec::new();
        }

        if state.repeat == RepeatMode::One {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(count);

        if state.shuffle {
            let start_pos = state.shuffle_position + 1;
            for i in 0..count {
                let pos = start_pos + i;
                if pos < state.shuffle_order.len() {
                    if let Some(track) = state.tracks.get(state.shuffle_order[pos]) {
                        result.push(track.clone());
                    }
                } else if state.repeat == RepeatMode::All {
                    let wrapped_pos = pos % state.shuffle_order.len();
                    if let Some(track) = state.tracks.get(state.shuffle_order[wrapped_pos]) {
                        result.push(track.clone());
                    }
                }
            }
        } else {
            let start_idx = state.current_index.map(|i| i + 1).unwrap_or(0);
            for i in 0..count {
                let idx = start_idx + i;
                if idx < state.tracks.len() {
                    result.push(state.tracks[idx].clone());
                } else if state.repeat == RepeatMode::All {
                    let wrapped_idx = idx % state.tracks.len();
                    result.push(state.tracks[wrapped_idx].clone());
                }
            }
        }

        result
    }

    /// Inspect the track [`Self::previous`] would select without consuming
    /// history or changing either queue cursor. Frontends use this before a
    /// fallible physical-file handoff: publishing the previous row first can
    /// leave metadata claiming a moved file while the old stream is audible.
    pub fn peek_previous(&self) -> Option<QueueTrack> {
        let state = self.state.lock().unwrap();
        if state.tracks.is_empty() {
            return None;
        }
        if let Some(&prev_idx) = state.history.back() {
            return state.tracks.get(prev_idx).cloned();
        }
        let prev_idx = if state.shuffle {
            if state.shuffle_position > 0 {
                state.shuffle_order.get(state.shuffle_position - 1).copied()
            } else if state.repeat == RepeatMode::All {
                state.shuffle_order.last().copied()
            } else {
                state.shuffle_order.first().copied()
            }
        } else {
            let curr_idx = state.current_index.unwrap_or(0);
            if curr_idx > 0 {
                Some(curr_idx - 1)
            } else if state.repeat == RepeatMode::All {
                Some(state.tracks.len().saturating_sub(1))
            } else {
                Some(0)
            }
        };
        prev_idx.and_then(|idx| state.tracks.get(idx).cloned())
    }

    /// Advance to next track and return it
    pub fn next(&self) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();
        if state.tracks.is_empty() {
            return None;
        }

        if state.repeat == RepeatMode::One {
            return state
                .current_index
                .and_then(|idx| state.tracks.get(idx).cloned());
        }

        // Save current to history before moving
        if let Some(curr_idx) = state.current_index {
            Self::record_history_internal(&mut state, curr_idx);
        }

        let next_idx = if state.shuffle {
            state.shuffle_position += 1;
            if state.shuffle_position < state.shuffle_order.len() {
                Some(state.shuffle_order[state.shuffle_position])
            } else if state.repeat == RepeatMode::All {
                state.shuffle_position = 0;
                state.shuffle_order.first().copied()
            } else {
                None
            }
        } else {
            let curr_idx = state.current_index.unwrap_or(0);
            let next_idx = curr_idx + 1;
            if next_idx < state.tracks.len() {
                Some(next_idx)
            } else if state.repeat == RepeatMode::All {
                Some(0)
            } else {
                None
            }
        };

        if let Some(next_index) = next_idx {
            state.history.retain(|&index| index != next_index);
        }
        state.current_index = next_idx;
        // Manual-block bookkeeping (#442): advancing past a manual entry
        // shrinks the block; exhausting or wrapping the queue dissolves it.
        match next_idx {
            Some(0) | None => state.manual_next_count = 0,
            Some(_) => state.manual_next_count = state.manual_next_count.saturating_sub(1),
        }
        next_idx.and_then(|idx| state.tracks.get(idx).cloned())
    }

    /// Go to previous track and return it
    pub fn previous(&self) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();
        if state.tracks.is_empty() {
            return None;
        }

        // Try to get from history first
        if let Some(prev_idx) = state.history.pop_back() {
            state.history.retain(|&index| index != prev_idx);
            state.current_index = Some(prev_idx);

            if state.shuffle {
                if let Some(pos) = state.shuffle_order.iter().position(|&x| x == prev_idx) {
                    state.shuffle_position = pos;
                }
            }

            return state.tracks.get(prev_idx).cloned();
        }

        // No history, go to previous in order
        let prev_idx = if state.shuffle {
            if state.shuffle_position > 0 {
                state.shuffle_position -= 1;
                Some(state.shuffle_order[state.shuffle_position])
            } else if state.repeat == RepeatMode::All {
                state.shuffle_position = state.shuffle_order.len().saturating_sub(1);
                state.shuffle_order.last().copied()
            } else {
                state.shuffle_order.first().copied()
            }
        } else {
            let curr_idx = state.current_index.unwrap_or(0);
            if curr_idx > 0 {
                Some(curr_idx - 1)
            } else if state.repeat == RepeatMode::All {
                Some(state.tracks.len().saturating_sub(1))
            } else {
                Some(0)
            }
        };

        state.current_index = prev_idx;
        prev_idx.and_then(|idx| state.tracks.get(idx).cloned())
    }

    /// Move the current pointer to the track whose id matches `id`, WITHOUT
    /// starting playback. Used to reconcile the queue pointer to a track the
    /// audio engine already advanced to on its own (a gapless hand-off
    /// happens inside the player, not through `next`), so the now-playing
    /// card never goes stale while the seek bar keeps moving.
    ///
    /// Returns the matched track plus whether the pointer actually moved
    /// (`false` = it was already current). Returns `None` when no queue track
    /// has that id, leaving the pointer untouched.
    pub fn sync_current_to_id(&self, id: u64) -> Option<(QueueTrack, bool)> {
        let mut state = self.state.lock().unwrap();
        let target = state.tracks.iter().position(|t| t.id == id)?;
        let moved = state.current_index != Some(target);
        if moved {
            // Record the outgoing track so `previous` still walks back.
            if let Some(curr_idx) = state.current_index {
                Self::record_history_internal(&mut state, curr_idx);
            }
            state.history.retain(|&index| index != target);
            state.current_index = Some(target);
            // Keep the shuffle cursor aligned with the new position.
            if state.shuffle {
                if let Some(pos) = state.shuffle_order.iter().position(|&x| x == target) {
                    state.shuffle_position = pos;
                }
            }
        }
        state.tracks.get(target).cloned().map(|t| (t, moved))
    }

    /// Jump to a track by its position in the `upcoming` list as returned by
    /// `get_state`. This is the position the user sees in the Queue sidebar;
    /// the method resolves it to the correct canonical index even when
    /// shuffle is active (where the display order differs from the canonical
    /// `tracks` order).
    ///
    /// Used by the "click a track in the queue panel" path — fixes issue
    /// #327 where shuffle mode caused a different track than the one
    /// clicked to be played.
    pub fn play_upcoming_at(&self, upcoming_index: usize) -> Option<QueueTrack> {
        let canonical_index = {
            let state = self.state.lock().unwrap();
            match state.current_index {
                Some(_) if state.shuffle => state
                    .shuffle_order
                    .get(state.shuffle_position + 1 + upcoming_index)
                    .copied(),
                Some(curr_idx) => Some(curr_idx + 1 + upcoming_index),
                None => Some(upcoming_index),
            }
        };
        canonical_index.and_then(|idx| self.play_index(idx))
    }

    /// Move within the chronological Listen List without changing its leading
    /// edge. Unlike [`Self::play_upcoming_at`], every row crossed on the way
    /// to the target is appended to playback history in play order. The flat
    /// projection therefore keeps the same sequence and only moves its NOW
    /// cursor; ordinary album/playlist/queue-sidebar activation retains the
    /// existing jump semantics through the sibling method above.
    ///
    /// `expected_id` is checked under the same lock as the cursor mutation.
    /// Queue documents are snapshots, so an index that went stale between the
    /// click and this call must not activate a different row.
    pub fn play_upcoming_at_preserving_timeline(
        &self,
        upcoming_index: usize,
        expected_id: u64,
    ) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();
        if state.tracks.is_empty() {
            return None;
        }

        let (target_index, crossed): (usize, Vec<usize>) = match state.current_index {
            Some(_) if state.shuffle => {
                let target_position = state
                    .shuffle_position
                    .checked_add(1)?
                    .checked_add(upcoming_index)?;
                let target_index = *state.shuffle_order.get(target_position)?;
                let crossed = state.shuffle_order[state.shuffle_position..target_position].to_vec();
                (target_index, crossed)
            }
            Some(current_index) => {
                let target_index = current_index.checked_add(1)?.checked_add(upcoming_index)?;
                if target_index >= state.tracks.len() {
                    return None;
                }
                (target_index, (current_index..target_index).collect())
            }
            None => {
                let target_index = upcoming_index;
                if target_index >= state.tracks.len() {
                    return None;
                }
                // Starting midway through an idle Listen List still preserves
                // its leading rows: they become the cursor's back path.
                (target_index, (0..target_index).collect())
            }
        };

        if state.tracks.get(target_index).map(|track| track.id) != Some(expected_id) {
            return None;
        }
        let target_shuffle_position = if state.shuffle {
            Some(
                state
                    .shuffle_order
                    .iter()
                    .position(|&index| index == target_index)?,
            )
        } else {
            None
        };

        for crossed_index in crossed {
            Self::record_history_internal(&mut state, crossed_index);
        }
        state.history.retain(|&index| index != target_index);
        state.current_index = Some(target_index);
        state.manual_next_count = state
            .manual_next_count
            .saturating_sub(upcoming_index.saturating_add(1));
        if let Some(position) = target_shuffle_position {
            state.shuffle_position = position;
        }

        state.tracks.get(target_index).cloned()
    }

    /// Move the Listen List cursor back to a most-recent-first history row.
    /// This is the direct equivalent of invoking [`Self::previous`] until that
    /// row becomes current: newer history entries return to the upcoming side
    /// through the canonical/shuffle order and older history remains behind
    /// the cursor. No row is cloned or inserted, so the flat list neither
    /// duplicates nor changes its beginning.
    pub fn play_history_at_preserving_timeline(
        &self,
        history_index: usize,
        expected_id: u64,
    ) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();
        let target_position = state
            .history
            .len()
            .checked_sub(history_index.checked_add(1)?)?;
        let target_index = *state.history.get(target_position)?;
        if state.tracks.get(target_index).map(|track| track.id) != Some(expected_id) {
            return None;
        }
        let target_shuffle_position = if state.shuffle {
            Some(
                state
                    .shuffle_order
                    .iter()
                    .position(|&index| index == target_index)?,
            )
        } else {
            None
        };

        // `target_position` itself becomes NOW, so retain only rows older
        // than it. VecDeque history is oldest-first internally.
        state.history.truncate(target_position);
        state.current_index = Some(target_index);
        if let Some(position) = target_shuffle_position {
            state.shuffle_position = position;
        }
        state.tracks.get(target_index).cloned()
    }

    /// Jump to a specific track by index
    pub fn play_index(&self, index: usize) -> Option<QueueTrack> {
        let mut state = self.state.lock().unwrap();
        if index >= state.tracks.len() {
            return None;
        }

        // Save current to history — ONLY when actually moving to a DIFFERENT
        // track. Jumping to the index already current (e.g. the QConnect
        // controller's `materialize_remote_queue` re-aligning the cursor to the
        // same index via `play_index`, since the stopped local player makes the
        // alignment fire unconditionally) must NOT record a spurious "previous"
        // entry, or the current track shows up duplicated in the History tab.
        // Matches `sync_current_to_id`'s `moved` guard.
        if let Some(curr_idx) = state.current_index {
            if curr_idx != index {
                Self::record_history_internal(&mut state, curr_idx);
            }
        }

        state.history.retain(|&history_index| history_index != index);
        state.current_index = Some(index);

        if state.shuffle {
            if let Some(pos) = state.shuffle_order.iter().position(|&x| x == index) {
                state.shuffle_position = pos;
            }
        }

        state.tracks.get(index).cloned()
    }

    /// Toggle shuffle mode
    pub fn set_shuffle(&self, enabled: bool) {
        let mut state = self.state.lock().unwrap();
        if state.shuffle == enabled {
            return;
        }

        if enabled {
            state.shuffle = true;
            // The manual block is meaningless once the order is reshuffled (#442).
            state.manual_next_count = 0;
            Self::regenerate_shuffle_order_internal(&mut state);

            // Shuffle only the part of the linear queue that has not been
            // traversed yet. The canonical prefix stays behind the cursor, so
            // enabling shuffle midway through an album cannot replay tracks
            // the user already passed.
            if let Some(curr_idx) = state.current_index {
                let mut order = (0..=curr_idx).collect::<Vec<_>>();
                order.extend(
                    state
                        .shuffle_order
                        .iter()
                        .copied()
                        .filter(|&idx| idx > curr_idx),
                );
                state.shuffle_order = order;
                state.shuffle_position = curr_idx;
            }
        } else {
            // Turning shuffle off is a playback-order transition, not merely
            // a flag change. If the shuffled cursor is on canonical track 8,
            // deriving the ordinary tail from index 8 would silently discard
            // every unheard track whose canonical index is lower. Flatten the
            // traversed shuffle prefix + NOW + every still-unheard canonical
            // occurrence into the physical queue before returning to linear
            // traversal. History is remapped by occurrence index below and
            // therefore keeps its chronology and duplicate rows intact.
            Self::linearize_after_shuffle_internal(&mut state);
            state.shuffle = false;
        }
    }

    /// Set shuffle mode using an authoritative order produced elsewhere.
    /// Used by QConnect so the local queue follows the remote session order
    /// instead of generating a second independent shuffle.
    pub fn set_shuffle_with_order(&self, enabled: bool, shuffle_order: Option<Vec<usize>>) {
        let mut state = self.state.lock().unwrap();

        if !enabled {
            if state.shuffle {
                Self::linearize_after_shuffle_internal(&mut state);
            } else {
                state.shuffle_order.clear();
                state.shuffle_position = 0;
            }
            state.shuffle = false;
            return;
        }

        state.shuffle = true;

        if let Some(order) =
            shuffle_order.filter(|order| Self::is_valid_shuffle_order(order, state.tracks.len()))
        {
            state.shuffle_order = order;
            if let Some(curr_idx) = state.current_index {
                if let Some(pos) = state.shuffle_order.iter().position(|&idx| idx == curr_idx) {
                    state.shuffle_position = pos;
                } else {
                    state.shuffle_position = 0;
                }
            } else {
                state.shuffle_position = 0;
            }
            return;
        }

        Self::set_identity_shuffle_order_internal(&mut state);
    }

    /// Get shuffle status
    pub fn is_shuffle(&self) -> bool {
        self.state.lock().unwrap().shuffle
    }

    /// Set repeat mode
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.state.lock().unwrap().repeat = mode;
    }

    /// Get repeat mode
    pub fn get_repeat(&self) -> RepeatMode {
        self.state.lock().unwrap().repeat
    }

    /// Set the "stop after" marker on a specific track ID. Replaces any
    /// previous marker. Silent no-op if the track ID is not currently in
    /// the queue (defensive check — frontend should only ever pass IDs
    /// from the current queue).
    pub fn set_stop_after(&self, track_id: u64) {
        let mut state = self.state.lock().unwrap();
        if state.tracks.iter().any(|t| t.id == track_id) {
            state.stop_after_track_id = Some(track_id);
        }
    }

    /// Clear the marker (user cancellation from UI).
    pub fn clear_stop_after(&self) {
        let mut state = self.state.lock().unwrap();
        state.stop_after_track_id = None;
    }

    /// Read current marker (used by `get_state()` for serialization).
    pub fn get_stop_after(&self) -> Option<u64> {
        self.state.lock().unwrap().stop_after_track_id
    }

    /// One-shot consume: if the finished track ID matches the marker,
    /// clear it and return true. Otherwise return false. The
    /// auto-advance driver calls this on every natural track-end and
    /// pauses (instead of advancing) when it returns true. Manual skip
    /// paths must NOT call this.
    pub fn consume_stop_after_if(&self, finished_track_id: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.stop_after_track_id == Some(finished_track_id) {
            state.stop_after_track_id = None;
            true
        } else {
            false
        }
    }

    /// Get queue state for frontend
    pub fn get_state(&self) -> QueueState {
        let state = self.state.lock().unwrap();

        let current_track = state
            .current_index
            .and_then(|idx| state.tracks.get(idx).cloned());

        // Get upcoming tracks (after current)
        let upcoming: Vec<QueueTrack> = if let Some(curr_idx) = state.current_index {
            if state.shuffle {
                state
                    .shuffle_order
                    .iter()
                    .skip(state.shuffle_position + 1)
                    .take(20)
                    .filter_map(|&idx| state.tracks.get(idx).cloned())
                    .collect()
            } else {
                state
                    .tracks
                    .iter()
                    .skip(curr_idx + 1)
                    .take(20)
                    .cloned()
                    .collect()
            }
        } else {
            state.tracks.iter().take(20).cloned().collect()
        };

        // Get history tracks (recent first)
        let history_tracks: Vec<QueueTrack> = state
            .history
            .iter()
            .rev()
            .take(10)
            .filter_map(|&idx| state.tracks.get(idx).cloned())
            .collect();

        QueueState {
            current_track,
            current_index: state.current_index,
            upcoming,
            history: history_tracks,
            shuffle: state.shuffle,
            repeat: state.repeat,
            total_tracks: state.tracks.len(),
            stop_after_track_id: state.stop_after_track_id,
            manual_next_count: state.manual_next_count,
        }
    }

    /// Get all tracks in the queue plus the current index (for session persistence).
    /// Unlike get_state() which caps upcoming/history, this returns the full track list.
    pub fn get_all_tracks(&self) -> (Vec<QueueTrack>, Option<usize>) {
        let state = self.state.lock().unwrap();
        (state.tracks.clone(), state.current_index)
    }

    /// Capture the complete queue authority state under one lock.
    ///
    /// Unlike the frontend and persistence projections, this preserves every
    /// playback-order detail without caps or normalization: canonical tracks,
    /// current cursor, shuffle order and cursor, repeat mode, chronological
    /// history, stop-after marker, and the manual-next block size.
    pub fn capture_authority_snapshot(&self) -> QueueAuthoritySnapshot {
        let state = self.state.lock().unwrap();
        QueueAuthoritySnapshot(state.clone())
    }

    /// Atomically replace the complete queue authority state with `snapshot`.
    ///
    /// Consuming the opaque snapshot prevents callers from restoring only a
    /// subset of the coupled queue fields or mutating them between capture and
    /// restore.
    pub fn restore_authority_snapshot(&self, snapshot: QueueAuthoritySnapshot) {
        let mut state = self.state.lock().unwrap();
        *state = snapshot.0;
    }

    /// Capture the queue rows, cursor, and chronological play-history indices
    /// under one lock for durable session persistence.
    ///
    /// History is stored oldest-first internally. Its indices refer to the
    /// returned `tracks` vector, so callers must persist the three values as a
    /// single snapshot.
    pub fn get_persistable_state(&self) -> (Vec<QueueTrack>, Option<usize>, Vec<usize>) {
        let state = self.state.lock().unwrap();
        (
            state.tracks.clone(),
            state.current_index,
            state.history.iter().copied().collect(),
        )
    }

    /// Restore a persisted oldest-first play history for the current queue.
    /// Invalid indices are discarded and only the manager's newest 50 entries
    /// survive, matching the live playback-history bound.
    pub fn restore_history_indices(&self, history: Vec<usize>) {
        let mut state = self.state.lock().unwrap();
        state.history.clear();
        for index in history {
            if state.current_index != Some(index) {
                Self::record_history_internal(&mut state, index);
            }
        }
    }

    /// Get the full queue state without the upcoming/history caps applied by
    /// `get_state()`. Used by clients that paginate the upcoming list (e.g.
    /// the Queue sidebar's "UP NEXT" paginator) and need the complete history.
    /// The `upcoming` ordering is shuffle-aware, matching `get_state()`.
    pub fn get_state_full(&self) -> QueueState {
        let state = self.state.lock().unwrap();

        let current_track = state
            .current_index
            .and_then(|idx| state.tracks.get(idx).cloned());

        // Full upcoming list (after current), shuffle-aware. No `take` cap.
        let upcoming: Vec<QueueTrack> = if let Some(curr_idx) = state.current_index {
            if state.shuffle {
                state
                    .shuffle_order
                    .iter()
                    .skip(state.shuffle_position + 1)
                    .filter_map(|&idx| state.tracks.get(idx).cloned())
                    .collect()
            } else {
                state
                    .tracks
                    .iter()
                    .skip(curr_idx + 1)
                    .cloned()
                    .collect()
            }
        } else {
            state.tracks.clone()
        };

        // Full history (recent first). No `take` cap.
        let history_tracks: Vec<QueueTrack> = state
            .history
            .iter()
            .rev()
            .filter_map(|&idx| state.tracks.get(idx).cloned())
            .collect();

        QueueState {
            current_track,
            current_index: state.current_index,
            upcoming,
            history: history_tracks,
            shuffle: state.shuffle,
            repeat: state.repeat,
            total_tracks: state.tracks.len(),
            stop_after_track_id: state.stop_after_track_id,
            manual_next_count: state.manual_next_count,
        }
    }

    /// Regenerate shuffle order (internal, must be called with lock held)
    fn regenerate_shuffle_order_internal(state: &mut InternalState) {
        let mut order: Vec<usize> = (0..state.tracks.len()).collect();

        // Fisher-Yates shuffle with proper PRNG
        use rand::{RngExt, SeedableRng};
        use std::time::{SystemTime, UNIX_EPOCH};

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

        for i in (1..order.len()).rev() {
            let j = rng.random_range(0..=i);
            order.swap(i, j);
        }

        state.shuffle_order = order;

        if let Some(curr_idx) = state.current_index {
            if let Some(pos) = state.shuffle_order.iter().position(|&x| x == curr_idx) {
                state.shuffle_position = pos;
            } else {
                state.shuffle_position = 0;
            }
        } else {
            state.shuffle_position = 0;
        }
    }

    /// Preserve the existing queue order when shuffle is remote-controlled but
    /// no authoritative remote order has arrived yet.
    fn set_identity_shuffle_order_internal(state: &mut InternalState) {
        state.shuffle_order = (0..state.tracks.len()).collect();

        if let Some(curr_idx) = state.current_index {
            state.shuffle_position = curr_idx.min(state.shuffle_order.len().saturating_sub(1));
        } else {
            state.shuffle_position = 0;
        }
    }

    /// Convert the active shuffled timeline into a linear queue without
    /// losing unheard rows whose canonical indices precede the current one.
    ///
    /// The new physical order is:
    /// `traversed shuffle prefix · current · unheard rows in canonical order`.
    /// Every coordinate stored by the manager is then remapped from the old
    /// occurrence index to the new one. Track ids are deliberately irrelevant:
    /// two separately queued copies of the same recording must remain two
    /// separately positioned occurrences.
    fn linearize_after_shuffle_internal(state: &mut InternalState) {
        let track_count = state.tracks.len();
        let Some(current_index) = state.current_index.filter(|&idx| idx < track_count) else {
            state.shuffle_order.clear();
            state.shuffle_position = 0;
            return;
        };
        let Some(current_position) = state
            .shuffle_order
            .iter()
            .position(|&idx| idx == current_index)
        else {
            // Defensive fallback for a damaged/incomplete order: preserve the
            // physical queue rather than guessing which rows were traversed.
            state.shuffle_order.clear();
            state.shuffle_position = 0;
            return;
        };

        let mut seen = vec![false; track_count];
        let mut linear_order = Vec::with_capacity(track_count);
        for &index in state.shuffle_order.iter().take(current_position) {
            if index < track_count && !seen[index] {
                seen[index] = true;
                linear_order.push(index);
            }
        }
        if !seen[current_index] {
            seen[current_index] = true;
            linear_order.push(current_index);
        }
        for (index, was_seen) in seen.iter().enumerate() {
            if !was_seen {
                linear_order.push(index);
            }
        }

        let mut old_to_new = vec![None; track_count];
        for (new_index, &old_index) in linear_order.iter().enumerate() {
            old_to_new[old_index] = Some(new_index);
        }
        let mut slots = std::mem::take(&mut state.tracks)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        state.tracks = linear_order
            .iter()
            .filter_map(|&old_index| slots[old_index].take())
            .collect();
        state.current_index = old_to_new[current_index];
        state.history = state
            .history
            .iter()
            .filter_map(|&old_index| old_to_new.get(old_index).copied().flatten())
            .collect();
        state.shuffle_order.clear();
        state.shuffle_position = 0;
    }

    /// Record one canonical queue occurrence as the newest played entry.
    /// Replaying that SAME occurrence moves its existing record to the end;
    /// independently enqueued copies remain distinct because their canonical
    /// indices differ even when their track ids are equal.
    fn record_history_internal(state: &mut InternalState, index: usize) {
        if index >= state.tracks.len() {
            return;
        }
        state.history.retain(|&existing| existing != index);
        state.history.push_back(index);
        while state.history.len() > MAX_HISTORY_LEN {
            state.history.pop_front();
        }
    }

    /// Remap history entries from `state.tracks` indices to indices into
    /// `new_tracks`. Repeated track ids are matched occurrence-by-occurrence
    /// in canonical order rather than collapsed onto the last matching id.
    /// Entries whose occurrence no longer exists are dropped. Must be called
    /// with the lock held and BEFORE `state.tracks` is replaced.
    ///
    /// This preserves history across queue version bumps that don't change
    /// track identity (e.g. pure reorder, shuffle toggle, or an authoritative
    /// remote echo of the current local queue). Bug #316.
    fn remap_history_by_track_id_internal(state: &mut InternalState, new_tracks: &[QueueTrack]) {
        if state.history.is_empty() || new_tracks.is_empty() || state.tracks.is_empty() {
            state.history.clear();
            return;
        }

        // QueueTrack has no separate persisted occurrence UUID. Pairing the
        // nth old occurrence with the nth new occurrence is the strongest
        // stable identity available across an authoritative queue replacement
        // and, crucially, does not merge duplicate queue entries.
        let mut new_indices_by_id: HashMap<u64, VecDeque<usize>> =
            HashMap::with_capacity(new_tracks.len());
        for (idx, track) in new_tracks.iter().enumerate() {
            new_indices_by_id.entry(track.id).or_default().push_back(idx);
        }
        let mut old_to_new = vec![None; state.tracks.len()];
        for (old_index, track) in state.tracks.iter().enumerate() {
            old_to_new[old_index] = new_indices_by_id
                .get_mut(&track.id)
                .and_then(VecDeque::pop_front);
        }

        let mut remapped: VecDeque<usize> = VecDeque::with_capacity(state.history.len());
        for &old_idx in state.history.iter() {
            if let Some(new_idx) = old_to_new.get(old_idx).copied().flatten() {
                remapped.retain(|&existing| existing != new_idx);
                remapped.push_back(new_idx);
            }
        }
        while remapped.len() > MAX_HISTORY_LEN {
            remapped.pop_front();
        }
        state.history = remapped;
    }

    /// Remap an index after remove+insert move operation.
    fn remap_index_after_move(idx: usize, from_idx: usize, to_idx: usize) -> usize {
        if idx == from_idx {
            return to_idx;
        }

        if from_idx < to_idx {
            // Moved down: [from+1 ..= to] shift left
            if idx > from_idx && idx <= to_idx {
                idx - 1
            } else {
                idx
            }
        } else {
            // Moved up: [to .. from-1] shift right
            if idx >= to_idx && idx < from_idx {
                idx + 1
            } else {
                idx
            }
        }
    }

    /// Remove one absolute track index from shuffle order and rebase remaining indices.
    fn remove_index_from_shuffle_internal(state: &mut InternalState, removed_idx: usize) {
        if let Some(pos) = state
            .shuffle_order
            .iter()
            .position(|&idx| idx == removed_idx)
        {
            state.shuffle_order.remove(pos);

            if pos < state.shuffle_position && state.shuffle_position > 0 {
                state.shuffle_position -= 1;
            } else if pos == state.shuffle_position
                && state.shuffle_position >= state.shuffle_order.len()
            {
                state.shuffle_position = state.shuffle_order.len().saturating_sub(1);
            }
        }

        for idx in state.shuffle_order.iter_mut() {
            if *idx > removed_idx {
                *idx -= 1;
            }
        }

        if let Some(curr_idx) = state.current_index {
            if let Some(pos) = state.shuffle_order.iter().position(|&idx| idx == curr_idx) {
                state.shuffle_position = pos;
            } else {
                state.shuffle_position = 0;
            }
        } else {
            state.shuffle_position = 0;
        }
    }

    fn is_valid_shuffle_order(order: &[usize], track_count: usize) -> bool {
        if order.len() != track_count {
            return false;
        }

        let mut seen = vec![false; track_count];
        for &idx in order {
            if idx >= track_count || seen[idx] {
                return false;
            }
            seen[idx] = true;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_track(id: u64) -> QueueTrack {
        QueueTrack {
            id,
            title: format!("Track {}", id),
            version: None,
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            album_version: None,
            duration_secs: 180,
            artwork_url: None,
            hires: false,
            bit_depth: None,
            sample_rate: None,
            is_local: false,
            album_id: None,
            artist_id: None,
            streamable: true,
            source: Some("test".to_string()),
            parental_warning: false,
            source_item_id_hint: None,
            context_kind: None,
            context_id: None,
            isrc: None,
            recording_mbid: None,
        }
    }

    #[test]
    fn authority_snapshot_round_trips_complete_shuffled_state() {
        let queue = QueueManager::new();
        let mut tracks: Vec<_> = (101..=105).map(create_test_track).collect();
        tracks[2].version = Some("Authority snapshot edition".to_string());
        tracks[2].bit_depth = Some(24);
        tracks[2].sample_rate = Some(192.0);
        tracks[2].isrc = Some("TEST00000103".to_string());

        let shuffle_order = vec![4, 2, 0, 3, 1];
        queue.set_queue_with_order(tracks, Some(2), true, Some(shuffle_order.clone()));
        queue.play_index(3);
        queue.set_repeat(RepeatMode::All);
        queue.set_stop_after(105);

        let expected_tracks = {
            let state = queue.state.lock().unwrap();
            format!("{:?}", state.tracks)
        };
        let snapshot = queue.capture_authority_snapshot();

        queue.clear(false);
        queue.add_track(create_test_track(999));
        queue.play_index(0);
        queue.set_repeat(RepeatMode::One);

        queue.restore_authority_snapshot(snapshot);

        let state = queue.state.lock().unwrap();
        assert_eq!(format!("{:?}", state.tracks), expected_tracks);
        assert_eq!(state.current_index, Some(3));
        assert!(state.shuffle);
        assert_eq!(state.shuffle_order, shuffle_order);
        assert_eq!(state.shuffle_position, 3);
        assert_eq!(state.repeat, RepeatMode::All);
        assert_eq!(state.history, VecDeque::from([2]));
        assert_eq!(state.stop_after_track_id, Some(105));
        assert_eq!(state.manual_next_count, 0);
    }

    #[test]
    fn authority_snapshot_round_trips_manual_next_block() {
        let queue = QueueManager::new();
        queue.set_queue(
            vec![
                create_test_track(1),
                create_test_track(2),
                create_test_track(3),
            ],
            Some(0),
        );
        queue.add_track_next(create_test_track(10));
        queue.add_track_later(create_test_track(11));
        queue.set_repeat(RepeatMode::One);
        queue.set_stop_after(3);

        let (expected_shuffle_order, expected_shuffle_position) = {
            let state = queue.state.lock().unwrap();
            (state.shuffle_order.clone(), state.shuffle_position)
        };
        let snapshot = queue.capture_authority_snapshot();
        queue.clear(false);
        queue.restore_authority_snapshot(snapshot);

        let state = queue.state.lock().unwrap();
        assert_eq!(
            state
                .tracks
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![1, 10, 11, 2, 3]
        );
        assert_eq!(state.current_index, Some(0));
        assert!(!state.shuffle);
        assert_eq!(state.shuffle_order, expected_shuffle_order);
        assert_eq!(state.shuffle_position, expected_shuffle_position);
        assert_eq!(state.repeat, RepeatMode::One);
        assert!(state.history.is_empty());
        assert_eq!(state.stop_after_track_id, Some(3));
        assert_eq!(state.manual_next_count, 2);
    }

    #[test]
    fn repeat_all_wraps_a_local_album_for_renderer_advance() {
        let queue = QueueManager::new();
        for id in 1..=3 {
            let mut track = create_test_track(id);
            track.is_local = true;
            track.source = Some("local".to_string());
            queue.add_track(track);
        }
        queue.play_index(2);
        queue.set_repeat(RepeatMode::All);

        let wrapped = queue.next().expect("repeat-all must wrap the album");
        assert_eq!(wrapped.id, 1);
        assert_eq!(queue.current_track().map(|track| track.id), Some(1));
    }

    #[test]
    fn repeat_one_returns_the_same_local_track_for_renderer_advance() {
        let queue = QueueManager::new();
        let mut track = create_test_track(7);
        track.is_local = true;
        track.source = Some("local".to_string());
        queue.add_track(track);
        queue.play_index(0);
        queue.set_repeat(RepeatMode::One);

        let repeated = queue.next().expect("repeat-one must replay the current row");
        assert_eq!(repeated.id, 7);
        assert_eq!(queue.current_track().map(|track| track.id), Some(7));
    }

    #[test]
    fn test_clear_without_current_track() {
        let queue = QueueManager::new();

        queue.add_track(create_test_track(123));
        queue.add_track(create_test_track(124));
        queue.add_track(create_test_track(125));

        queue.clear(true);

        let state = queue.get_state();
        assert!(state.current_track.is_none());
        assert!(state.upcoming.is_empty());
        assert_eq!(state.total_tracks, 0);
    }

    #[test]
    fn test_clear_keeps_current_track() {
        let queue = QueueManager::new();

        queue.add_track(create_test_track(123));
        queue.add_track(create_test_track(124));
        queue.add_track(create_test_track(125));
        queue.play_index(0);

        queue.clear(true);

        let state = queue.get_state();
        assert!(state.current_track.is_some());
        assert_eq!(state.current_track.unwrap().id, 123);
        assert!(state.upcoming.is_empty());
        assert_eq!(state.total_tracks, 1);
    }

    /// Regression: clear(true) must keep the track at `current_index`, not
    /// always `tracks[0]`. Mid-album "Clear queue" previously left the first
    /// row as now-playing while audio kept the real current track.
    #[test]
    fn test_clear_keeps_mid_queue_current_track() {
        let queue = QueueManager::new();

        queue.add_track(create_test_track(100));
        queue.add_track(create_test_track(200));
        queue.add_track(create_test_track(300));
        queue.play_index(1); // current = 200

        queue.clear(true);

        let state = queue.get_state();
        assert!(state.current_track.is_some());
        assert_eq!(state.current_track.unwrap().id, 200);
        assert!(state.upcoming.is_empty());
        assert_eq!(state.total_tracks, 1);
    }

    #[test]
    fn test_clear_wipes_current_track_when_not_kept() {
        let queue = QueueManager::new();

        queue.add_track(create_test_track(123));
        queue.add_track(create_test_track(124));
        queue.play_index(0);

        // keep_current: false — user pressed Clear Queue while nothing was
        // actively playing, so the stale "now playing" slot should go too.
        queue.clear(false);

        let state = queue.get_state();
        assert!(state.current_track.is_none());
        assert!(state.upcoming.is_empty());
        assert_eq!(state.total_tracks, 0);
    }

    // ===================================================================
    // LANE C DIAGNOSTIC TESTS (B1 — sync_current_to_id pointer robustness)
    // These reproduce concrete states where the QueueManager pointer ends
    // up at a DIFFERENT track than the audio engine is actually playing.
    // ===================================================================

    /// B1 ROOT-CAUSE (a): live track id NOT present in `state.tracks`.
    /// When the audio engine has gaplessly advanced into a track that is no
    /// longer in the queue (e.g. the queue was replaced by a brand-new list
    /// while the OLD track was still draining, or an infinite-radio / single-
    /// track-replace), `sync_current_to_id` returns None and the pointer is
    /// left untouched — pointing at the STALE old track. Both NPB and sidebar
    /// then read the wrong `current_track`, and the poll loop never retries
    /// (it updates `last_track_id` and `continue`s). This is the "cleared the
    /// queue and it did NOT fix it" symptom.
    #[test]
    fn test_sync_current_to_id_unknown_id_leaves_pointer_stale() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(100)); // "Enjoy the Silence"
        queue.add_track(create_test_track(101));
        queue.play_index(0); // pointer = 0 -> track 100

        // The audio engine is actually playing track 999 ("Beetlebum"), which
        // is NOT in the queue (queue was replaced / radio handoff).
        let result = queue.sync_current_to_id(999);

        // Sync fails — pointer cannot be corrected.
        assert!(result.is_none(), "sync should return None for unknown id");

        // The pointer is STILL on the stale track 100, so the now-playing
        // surfaces report the WRONG track while audio plays 999.
        let state = queue.get_state_full();
        assert_eq!(
            state.current_track.unwrap().id,
            100,
            "pointer stays stale at the OLD track when sync_current_to_id can't find the live id"
        );
    }

    /// B1 ROOT-CAUSE (a'): clear(keep_current) when the kept track is NOT the
    /// audible one. The user clears the queue to "fix" the stale now-playing,
    /// but clear keeps the track at the (already stale) `current_index`, so the
    /// wrong track is now the SOLE entry — and because its id != the live audio
    /// id, a subsequent sync_current_to_id STILL can't correct it. Reproduces
    /// the user's "cleared the queue and it did NOT fix it".
    #[test]
    fn test_clear_keep_current_preserves_the_wrong_track() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(100)); // stale "now playing"
        queue.add_track(create_test_track(101));
        queue.play_index(0); // pointer stuck on 100 (audio actually plays 999)

        queue.clear(true); // user hits "Clear Queue" to fix the stale row

        let state = queue.get_state_full();
        // The kept track is the WRONG one (100), not the audible 999.
        assert_eq!(state.current_track.unwrap().id, 100);

        // And sync still can't fix it: 999 isn't in the 1-track queue.
        assert!(queue.sync_current_to_id(999).is_none());
        assert_eq!(queue.get_state_full().current_track.unwrap().id, 100);
    }

    /// B1 ROOT-CAUSE (b): DUPLICATE track ids in the queue.
    /// `sync_current_to_id` uses `position(|t| t.id == id)`, which returns the
    /// FIRST match. If the same track id appears twice (very common: a track
    /// added to a playlist twice, an album with a repeated bonus track, or a
    /// QConnect queue echo), advancing the audio into the SECOND occurrence
    /// resyncs the pointer to the FIRST occurrence — wrong index. The
    /// now-playing track id is "right" but the upcoming list, position, and any
    /// index-derived state are computed from the wrong slot.
    #[test]
    fn test_sync_current_to_id_duplicate_id_resyncs_to_first_occurrence() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(100)); // idx 0
        queue.add_track(create_test_track(200)); // idx 1
        queue.add_track(create_test_track(100)); // idx 2  (DUPLICATE of idx 0)
        queue.add_track(create_test_track(300)); // idx 3
        queue.play_index(1); // pointer = 1 (track 200)

        // Audio gaplessly advanced into the SECOND copy of track 100 (idx 2).
        let (_, moved) = queue
            .sync_current_to_id(100)
            .expect("id 100 is present, so sync succeeds");
        assert!(moved, "pointer moved off track 200");

        let state = queue.get_state_full();
        // current_index resynced to the FIRST occurrence (idx 0), NOT idx 2.
        assert_eq!(
            state.current_index,
            Some(0),
            "sync_current_to_id resyncs to the FIRST id match (idx 0), not the actually-playing idx 2"
        );
        // Consequence: the UP-NEXT list is computed from the wrong slot. From
        // idx 0 the upcoming is [200, 100, 300]; from the real idx 2 it should
        // be [300]. The sidebar shows a wrong upcoming list.
        let upcoming_ids: Vec<u64> = state.upcoming.iter().map(|t| t.id).collect();
        assert_eq!(
            upcoming_ids,
            vec![200, 100, 300],
            "upcoming is derived from the wrong (first-occurrence) index"
        );
    }

    #[test]
    fn test_clear_preserves_history() {
        let queue = QueueManager::new();

        queue.add_track(create_test_track(123));
        queue.add_track(create_test_track(124));
        queue.add_track(create_test_track(125));
        queue.play_index(0);
        queue.next(); // push 123 into history, current becomes 124

        let before = queue.get_state();
        assert_eq!(before.history.len(), 1);
        assert_eq!(before.history[0].id, 123);
        assert_eq!(before.current_track.as_ref().map(|t| t.id), Some(124));

        queue.clear(true);

        let after = queue.get_state();
        // Kept track is the one that was current (124), not tracks[0] (123).
        assert_eq!(after.current_track.as_ref().map(|t| t.id), Some(124));
        assert_eq!(after.total_tracks, 1);
        // History is index-based: entries for tracks that left the queue are
        // dropped (same remap-by-id path as set_queue). Under the old
        // truncate(1) bug, 123 stayed as the sole row so history still
        // "resolved" — that was an accident of the bug, not the contract.
        assert!(
            after.history.is_empty(),
            "history must not resolve removed tracks after clear: {:?}",
            after.history.iter().map(|t| t.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn play_index_to_same_track_does_not_push_history() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(1));
        queue.add_track(create_test_track(2));
        queue.play_index(0); // current None -> 0, no push

        // Re-align to the SAME index — the QConnect controller materialize path
        // calls play_index with the index already current. It must NOT record a
        // spurious "previous" entry (the track-shows-twice-in-History bug).
        queue.play_index(0);
        assert!(
            queue.get_state().history.is_empty(),
            "re-aligning to the current track must not add a history entry"
        );

        // A genuine move still records the outgoing track.
        queue.play_index(1);
        let history = queue.get_state().history;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, 1);
    }

    #[test]
    fn test_move_track_down_without_current_track() {
        let queue = QueueManager::new();

        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }

        let result = queue.move_track(0, 3);

        assert!(result, "move_track should succeed");
        assert_eq!(
            queue
                .get_state()
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<u64>>(),
            vec![2, 3, 1, 4, 5]
        );
    }

    #[test]
    fn test_move_track_down_with_current_track() {
        let queue = QueueManager::new();

        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(0);

        let result = queue.move_track(0, 3);

        assert!(result, "move_track should succeed");
        assert_eq!(
            queue
                .get_state()
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<u64>>(),
            vec![3, 4, 2, 5]
        );
    }

    #[test]
    fn test_move_track_up_without_current_track() {
        let queue = QueueManager::new();

        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }

        let result = queue.move_track(3, 0);

        assert!(result, "move_track should succeed");
        assert_eq!(
            queue
                .get_state()
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<u64>>(),
            vec![4, 1, 2, 3, 5]
        );
    }

    #[test]
    fn test_move_track_up_with_current_track() {
        let queue = QueueManager::new();

        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(0);

        let result = queue.move_track(3, 0);

        assert!(result, "move_track should succeed");
        assert_eq!(
            queue
                .get_state()
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<u64>>(),
            vec![5, 2, 3, 4]
        );
    }

    #[test]
    fn test_move_track_with_shuffle_reorders_shuffle_timeline() {
        let queue = QueueManager::new();
        for i in 1..=8 {
            queue.add_track(create_test_track(i));
        }

        queue.play_index(0);
        queue.set_shuffle(true);

        let before_shuffle = {
            let state = queue.state.lock().unwrap();
            state.shuffle_order.clone()
        };

        // With current_index=0 and shuffle_position=0:
        // upcoming move 2 -> 0 maps to shuffle positions 3 -> 1.
        assert!(queue.move_track(2, 0));

        let after_shuffle = {
            let state = queue.state.lock().unwrap();
            state.shuffle_order.clone()
        };

        let mut expected = before_shuffle.clone();
        let moved = expected.remove(3);
        expected.insert(1, moved);

        assert_eq!(after_shuffle, expected);
        assert_eq!(after_shuffle.len(), 8);
    }

    #[test]
    fn test_remove_track_with_shuffle_preserves_shuffle_order() {
        let queue = QueueManager::new();
        for i in 1..=8 {
            queue.add_track(create_test_track(i));
        }

        queue.play_index(0);
        queue.set_shuffle(true);

        let before_shuffle = {
            let state = queue.state.lock().unwrap();
            state.shuffle_order.clone()
        };

        assert!(queue.remove_track(2).is_some());

        let after_shuffle = {
            let state = queue.state.lock().unwrap();
            state.shuffle_order.clone()
        };

        let expected: Vec<usize> = before_shuffle
            .into_iter()
            .filter(|&idx| idx != 2)
            .map(|idx| if idx > 2 { idx - 1 } else { idx })
            .collect();

        assert_eq!(after_shuffle, expected);
        assert_eq!(after_shuffle.len(), 7);
    }

    #[test]
    fn test_enabling_shuffle_keeps_all_remaining_tracks_upcoming() {
        let queue = QueueManager::new();
        for i in 1..=11 {
            queue.add_track(create_test_track(i));
        }

        queue.play_index(0);
        queue.set_shuffle(true);

        let state = queue.get_state();
        assert_eq!(state.total_tracks, 11);
        assert_eq!(state.upcoming.len(), 10);
    }

    #[test]
    fn disabling_shuffle_keeps_every_unheard_track_upcoming() {
        let queue = QueueManager::new();
        queue.set_queue_with_order(
            (1..=9).map(create_test_track).collect(),
            Some(0),
            true,
            Some(vec![0, 7, 3, 1, 5, 8, 2, 6, 4]),
        );

        assert_eq!(queue.next().expect("shuffled track 8").id, 8);
        queue.set_shuffle(false);

        let state = queue.get_state_full();
        assert!(!state.shuffle);
        assert_eq!(state.total_tracks, 9);
        assert_eq!(state.current_track.expect("current track").id, 8);
        assert_eq!(
            state
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5, 6, 7, 9]
        );

        let mut played_after_unshuffle = Vec::new();
        while let Some(track) = queue.next() {
            played_after_unshuffle.push(track.id);
        }
        assert_eq!(played_after_unshuffle, vec![2, 3, 4, 5, 6, 7, 9]);
    }

    #[test]
    fn authoritative_shuffle_disable_keeps_every_unheard_track_upcoming() {
        let queue = QueueManager::new();
        queue.set_queue_with_order(
            (1..=6).map(create_test_track).collect(),
            Some(0),
            true,
            Some(vec![0, 4, 2, 5, 1, 3]),
        );

        assert_eq!(queue.next().expect("shuffled track 5").id, 5);
        queue.set_shuffle_with_order(false, None);

        let state = queue.get_state_full();
        assert!(!state.shuffle);
        assert_eq!(state.current_track.expect("current track").id, 5);
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 6]
        );
    }

    #[test]
    fn disabling_shuffle_remaps_duplicate_occurrences_without_merging_them() {
        let queue = QueueManager::new();
        let mut first_copy = create_test_track(7);
        first_copy.title = "First copy".into();
        let mut second_copy = create_test_track(7);
        second_copy.title = "Second copy".into();
        queue.set_queue_with_order(
            vec![
                first_copy,
                create_test_track(8),
                second_copy,
                create_test_track(9),
            ],
            Some(0),
            true,
            Some(vec![0, 2, 1, 3]),
        );

        assert_eq!(queue.next().expect("second copy").title, "Second copy");
        queue.set_shuffle(false);

        let state = queue.get_state_full();
        assert_eq!(state.total_tracks, 4);
        assert_eq!(state.history[0].title, "First copy");
        assert_eq!(state.current_track.expect("current copy").title, "Second copy");
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![8, 9]
        );
    }

    #[test]
    fn enabling_shuffle_mid_queue_does_not_requeue_the_traversed_prefix() {
        let queue = QueueManager::new();
        queue.set_queue((1..=7).map(create_test_track).collect(), Some(3));

        queue.set_shuffle(true);

        let state = queue.get_state_full();
        assert!(state.shuffle);
        assert_eq!(state.current_track.expect("current track").id, 4);
        assert_eq!(state.upcoming.len(), 3);
        assert!(state.upcoming.iter().all(|track| track.id > 4));
    }

    #[test]
    fn test_play_upcoming_at_without_shuffle_uses_linear_offset() {
        let queue = QueueManager::new();
        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(1); // current = track id 2

        // upcoming list is [3, 4, 5]; clicking position 1 must play id 4
        let track = queue.play_upcoming_at(1).expect("track");
        assert_eq!(track.id, 4);
    }

    #[test]
    fn idle_queue_promotes_first_row_for_transport_play() {
        let queue = QueueManager::new();
        queue.set_queue(vec![create_test_track(99)], Some(0));
        queue.clear(false);

        // All three enqueue actions keep an idle list cursor-less. "Later"
        // and "Next" form the manual block ahead of a plain queued row.
        queue.add_track(create_test_track(1));
        queue.add_track_later(create_test_track(2));
        queue.add_track_next(create_test_track(3));

        let idle = queue.get_state_full();
        assert!(idle.current_track.is_none());
        assert_eq!(
            idle.upcoming.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        let promoted = queue
            .play_upcoming_at_preserving_timeline(0, 3)
            .expect("first idle row");
        assert_eq!(promoted.id, 3);

        let playing = queue.get_state_full();
        assert_eq!(playing.current_track.expect("current").id, 3);
        assert_eq!(
            playing
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!(playing.history.is_empty());
        assert_eq!(playing.manual_next_count, 1);
    }

    #[test]
    fn peek_previous_matches_previous_without_consuming_history() {
        let queue = QueueManager::new();
        for i in 1..=4 {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(0);
        assert_eq!(queue.next().expect("next").id, 2);
        assert_eq!(queue.next().expect("next").id, 3);

        assert_eq!(queue.peek_previous().expect("peek").id, 2);
        assert_eq!(queue.peek_previous().expect("second peek").id, 2);
        assert_eq!(queue.previous().expect("previous").id, 2);
    }

    #[test]
    fn test_play_upcoming_at_with_shuffle_follows_shuffle_order() {
        let queue = QueueManager::new();
        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }

        // Authoritative shuffle: playing head is shuffle[0]=2 (id 3),
        // upcoming order becomes [5, 2, 4, 1] (track ids).
        queue.set_queue_with_order(
            (1..=5).map(create_test_track).collect(),
            Some(2),
            true,
            Some(vec![2, 4, 1, 3, 0]),
        );

        let state = queue.get_state();
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![5, 2, 4, 1]
        );

        // Clicking upcoming position 2 must play track id 4, not id 5
        // (which would be the "current_index + 2 + 1" = 5 broken path).
        let track = queue.play_upcoming_at(2).expect("track");
        assert_eq!(track.id, 4);
    }

    #[test]
    fn listen_list_upcoming_activation_preserves_linear_projection() {
        let queue = QueueManager::new();
        queue.set_queue((1..=6).map(create_test_track).collect(), Some(1));

        // Before: [2 NOW, 3, 4, 5, 6]. Selecting 5 must only move NOW.
        let track = queue
            .play_upcoming_at_preserving_timeline(2, 5)
            .expect("track");
        assert_eq!(track.id, 5);

        let state = queue.get_state_full();
        assert_eq!(state.current_track.expect("current").id, 5);
        assert_eq!(
            state
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![4, 3, 2]
        );
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![6]
        );
    }

    #[test]
    fn listen_list_upcoming_activation_preserves_shuffle_projection() {
        let queue = QueueManager::new();
        queue.set_queue_with_order(
            (1..=5).map(create_test_track).collect(),
            Some(2),
            true,
            Some(vec![2, 4, 1, 3, 0]),
        );

        // Before: [3 NOW, 5, 2, 4, 1]. Selecting 4 must retain 5 and 2
        // before the cursor instead of dropping them from the flat list.
        let track = queue
            .play_upcoming_at_preserving_timeline(2, 4)
            .expect("track");
        assert_eq!(track.id, 4);

        let state = queue.get_state_full();
        assert_eq!(state.current_track.expect("current").id, 4);
        assert_eq!(
            state
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 5, 3]
        );
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn listen_list_history_activation_moves_cursor_without_inserting() {
        let queue = QueueManager::new();
        queue.set_queue((1..=5).map(create_test_track).collect(), Some(3));
        queue.restore_history_indices(vec![0, 1, 2]);

        // Core history is exposed newest-first as [3, 2, 1]. Selecting its
        // middle row restores [1, 2 NOW, 3, 4, 5] without cloning track 2.
        let track = queue
            .play_history_at_preserving_timeline(1, 2)
            .expect("track");
        assert_eq!(track.id, 2);

        let state = queue.get_state_full();
        assert_eq!(state.current_track.expect("current").id, 2);
        assert_eq!(
            state
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert_eq!(state.total_tracks, 5);
    }

    #[test]
    fn replaying_the_same_queue_occurrence_keeps_only_its_latest_history_entry() {
        let queue = QueueManager::new();
        queue.set_queue((1..=3).map(create_test_track).collect(), Some(0));

        assert_eq!(queue.next().expect("track 2").id, 2);
        assert_eq!(queue.play_index(0).expect("replay track 1").id, 1);
        assert_eq!(
            queue
                .get_state_full()
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2],
            "the occurrence being replayed must leave History while current"
        );

        assert_eq!(queue.next().expect("track 2 again").id, 2);
        assert_eq!(queue.next().expect("track 3").id, 3);
        let history = queue.get_state_full().history;
        assert_eq!(
            history.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(history.iter().filter(|track| track.id == 1).count(), 1);
    }

    #[test]
    fn separately_enqueued_copies_keep_distinct_history_occurrences() {
        let queue = QueueManager::new();
        queue.set_queue(
            vec![
                create_test_track(7),
                create_test_track(8),
                create_test_track(7),
                create_test_track(9),
            ],
            Some(0),
        );

        queue.next();
        queue.next();
        queue.next();

        let history = queue.get_state_full().history;
        assert_eq!(
            history.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![7, 8, 7]
        );
        assert_eq!(history.iter().filter(|track| track.id == 7).count(), 2);
        assert_eq!(queue.get_persistable_state().2, vec![0, 1, 2]);
    }

    #[test]
    fn queue_view_reorders_history_in_chronological_slot_space() {
        let queue = QueueManager::new();
        queue.set_queue((1..=4).map(create_test_track).collect(), Some(3));
        queue.restore_history_indices(vec![0, 1, 2]);

        // Visible History starts [1, 2, 3] (oldest first). Drag 1 to the
        // trailing history gap, immediately before NOW.
        assert!(queue.move_history_entry(2, 1, 3));
        assert_eq!(
            queue
                .get_state_full()
                .history
                .iter()
                .rev()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert_eq!(queue.get_persistable_state().2, vec![1, 2, 0]);
    }

    #[test]
    fn queue_view_requeues_the_same_history_occurrence_without_cloning() {
        let queue = QueueManager::new();
        queue.set_queue((1..=5).map(create_test_track).collect(), Some(3));
        queue.restore_history_indices(vec![0, 1, 2]);

        // History is publicly [3, 2, 1]. Move occurrence 1 to Upcoming slot
        // 1: [2, 3, 4 NOW, 5, 1]. No sixth QueueTrack is created.
        assert!(queue.requeue_history_entry(2, 1, 1));
        let moved = queue.get_state_full();
        assert_eq!(moved.total_tracks, 5);
        assert_eq!(moved.current_track.expect("current").id, 4);
        assert_eq!(
            moved
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![3, 2]
        );
        assert_eq!(
            moved
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![5, 1]
        );

        queue
            .play_upcoming_at_preserving_timeline(1, 1)
            .expect("requeued occurrence");
        assert_eq!(
            queue
                .get_state_full()
                .history
                .iter()
                .filter(|track| track.id == 1)
                .count(),
            0,
            "the replayed occurrence is current, not also historical"
        );
        assert!(queue.next().is_none());
        let finished = queue.get_state_full();
        assert_eq!(finished.total_tracks, 5);
        assert_eq!(finished.history.iter().filter(|track| track.id == 1).count(), 1);
    }

    #[test]
    fn queue_view_requeues_only_the_selected_duplicate_occurrence() {
        let queue = QueueManager::new();
        let mut first_lunch = create_test_track(7);
        first_lunch.title = "Lunch A".into();
        let mut second_lunch = create_test_track(7);
        second_lunch.title = "Lunch B".into();
        queue.set_queue(
            vec![
                first_lunch,
                create_test_track(8),
                second_lunch,
                create_test_track(9),
                create_test_track(10),
            ],
            Some(3),
        );
        queue.restore_history_indices(vec![0, 1, 2]);

        // Most-recent History row is Lunch B. Its id equals Lunch A, so only
        // the phase coordinate/canonical occurrence can distinguish them.
        assert!(queue.requeue_history_entry(0, 7, 0));
        let state = queue.get_state_full();
        assert_eq!(state.total_tracks, 5);
        assert_eq!(state.history.iter().filter(|track| track.id == 7).count(), 1);
        assert_eq!(state.history[1].title, "Lunch A");
        assert_eq!(state.upcoming[0].title, "Lunch B");
    }

    #[test]
    fn queue_view_requeues_history_inside_the_shuffle_timeline() {
        let queue = QueueManager::new();
        queue.set_queue_with_order(
            (1..=5).map(create_test_track).collect(),
            Some(2),
            true,
            Some(vec![0, 1, 2, 3, 4]),
        );
        queue.restore_history_indices(vec![0, 1]);

        assert!(queue.requeue_history_entry(0, 2, 1));
        let state = queue.get_state_full();
        assert_eq!(state.total_tracks, 5);
        assert_eq!(state.current_track.expect("current").id, 3);
        assert_eq!(
            state
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![4, 2, 5]
        );
    }

    #[test]
    fn listen_list_activation_rejects_a_stale_row_atomically() {
        let queue = QueueManager::new();
        queue.set_queue((1..=4).map(create_test_track).collect(), Some(0));

        assert!(queue.play_upcoming_at_preserving_timeline(1, 999).is_none());

        let state = queue.get_state_full();
        assert_eq!(state.current_track.expect("current").id, 1);
        assert!(state.history.is_empty());
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn test_set_shuffle_with_order_uses_authoritative_order() {
        let queue = QueueManager::new();
        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }

        queue.play_index(2);
        queue.set_shuffle_with_order(true, Some(vec![2, 4, 1, 3, 0]));

        let state = queue.get_state();
        assert!(state.shuffle);
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![5, 2, 4, 1]
        );
    }

    #[test]
    fn test_set_shuffle_with_order_preserves_current_order_when_invalid() {
        let queue = QueueManager::new();
        for i in 1..=4 {
            queue.add_track(create_test_track(i));
        }

        queue.play_index(1);
        queue.set_shuffle_with_order(true, Some(vec![1, 1, 2, 3]));

        let state = queue.get_state();
        assert!(state.shuffle);
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    #[test]
    fn test_set_queue_with_order_applies_authoritative_shuffle_before_snapshot() {
        let queue = QueueManager::new();
        let tracks = (1..=5).map(create_test_track).collect::<Vec<_>>();

        queue.set_queue_with_order(tracks, Some(0), true, Some(vec![0, 3, 1, 4, 2]));

        let state = queue.get_state();
        assert!(state.shuffle);
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![4, 2, 5, 3]
        );
    }

    #[test]
    fn test_set_queue_with_order_preserves_queue_order_when_authoritative_order_missing() {
        let queue = QueueManager::new();
        let tracks = (1..=5).map(create_test_track).collect::<Vec<_>>();

        queue.set_queue_with_order(tracks, Some(1), true, None);

        let state = queue.get_state();
        assert!(state.shuffle);
        assert_eq!(
            state
                .upcoming
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
    }

    // --- Bug #316 history-preservation regression tests ---

    /// Helper: build a queue with N tracks, play track 0, advance through
    /// `advance_count` to populate history, returning the queue.
    fn queue_with_played_history(track_count: u64, advance_count: usize) -> QueueManager {
        let queue = QueueManager::new();
        for i in 1..=track_count {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(0);
        for _ in 0..advance_count {
            queue.next();
        }
        queue
    }

    #[test]
    fn test_set_queue_with_order_preserves_history_on_pure_reorder() {
        // Played 3 tracks, current is on track 4 (id=4).
        let queue = queue_with_played_history(5, 3);
        let before = queue.get_state();
        assert_eq!(
            before.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        // Same tracks, completely reordered. Current track (id=4) at new index 0.
        let reordered = vec![
            create_test_track(4),
            create_test_track(2),
            create_test_track(5),
            create_test_track(1),
            create_test_track(3),
        ];
        queue.set_queue_with_order(reordered, Some(0), false, None);

        let after = queue.get_state();
        // History rendered newest-first; ids must survive the reorder identically.
        assert_eq!(
            after.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn test_set_queue_with_order_preserves_history_when_tracks_added() {
        // Played track 1, then 2; current is track 3.
        let queue = queue_with_played_history(3, 2);

        // Same tracks plus 2 new ones (4, 5). Current still on track 3 (new index 2).
        let expanded = vec![
            create_test_track(1),
            create_test_track(2),
            create_test_track(3),
            create_test_track(4),
            create_test_track(5),
        ];
        queue.set_queue_with_order(expanded, Some(2), false, None);

        let after = queue.get_state();
        assert_eq!(
            after.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn test_set_queue_with_order_drops_only_removed_tracks_from_history() {
        // Played tracks 1, 2, 3; current on track 4 (id=4).
        let queue = queue_with_played_history(5, 3);
        let before = queue.get_state();
        assert_eq!(
            before.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        // Remove track id=2 from queue; tracks 1 and 3 survive in history.
        let trimmed = vec![
            create_test_track(1),
            create_test_track(3),
            create_test_track(4),
            create_test_track(5),
        ];
        queue.set_queue_with_order(trimmed, Some(2), false, None);

        let after = queue.get_state();
        assert_eq!(
            after.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 1]
        );
    }

    #[test]
    fn test_set_queue_with_order_clears_history_when_tracks_completely_different() {
        let queue = queue_with_played_history(5, 3);
        assert_eq!(queue.get_state().history.len(), 3);

        // No overlap with the previous queue; history must drop entirely.
        let fresh = vec![
            create_test_track(100),
            create_test_track(101),
            create_test_track(102),
        ];
        queue.set_queue_with_order(fresh, Some(0), false, None);

        let after = queue.get_state();
        assert!(after.history.is_empty());
    }

    #[test]
    fn test_set_queue_preserves_history_on_pure_reorder() {
        // Mirror test for set_queue (non-with-order variant).
        let queue = queue_with_played_history(5, 3);
        let before = queue.get_state();
        assert_eq!(
            before.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        let reordered = vec![
            create_test_track(5),
            create_test_track(4),
            create_test_track(3),
            create_test_track(2),
            create_test_track(1),
        ];
        queue.set_queue(reordered, Some(1)); // current track 4 now at idx 1

        let after = queue.get_state();
        assert_eq!(
            after.history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn test_set_queue_with_order_remaps_history_indices_after_reorder() {
        // Verify that after a reorder, the internal indices stored in history
        // actually point to the right new tracks (not just that ids match
        // through the get_state() projection accidentally).
        let queue = queue_with_played_history(4, 3);

        // Reverse order. Old tracks 1,2,3,4 -> new tracks 4,3,2,1.
        // Old history: indices [0, 1, 2] -> ids [1, 2, 3].
        // New mapping: id=1->idx 3, id=2->idx 2, id=3->idx 1.
        // Expected new history indices: [3, 2, 1] (front-to-back).
        let reversed = vec![
            create_test_track(4),
            create_test_track(3),
            create_test_track(2),
            create_test_track(1),
        ];
        queue.set_queue_with_order(reversed, Some(0), false, None);

        // Inspect internal state to verify the indices, not just rendered ids.
        let state = queue.state.lock().unwrap();
        assert_eq!(
            state.history.iter().copied().collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn queue_replacement_does_not_merge_duplicate_track_occurrences() {
        let queue = QueueManager::new();
        queue.set_queue(
            vec![
                create_test_track(7),
                create_test_track(8),
                create_test_track(7),
                create_test_track(9),
            ],
            Some(3),
        );
        queue.restore_history_indices(vec![0, 2]);

        queue.set_queue_with_order(
            vec![
                create_test_track(7),
                create_test_track(7),
                create_test_track(8),
                create_test_track(9),
            ],
            Some(3),
            false,
            None,
        );

        assert_eq!(queue.get_persistable_state().2, vec![0, 1]);
        assert_eq!(
            queue
                .get_state_full()
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![7, 7]
        );
    }

    // ============ Stop-After Marker — Basic API ============

    #[test]
    fn test_set_stop_after_stores_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.add_track(create_test_track(103));

        queue.set_stop_after(102);

        assert_eq!(queue.get_stop_after(), Some(102));
    }

    #[test]
    fn test_set_stop_after_replaces_previous_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));

        queue.set_stop_after(101);
        queue.set_stop_after(102);

        assert_eq!(queue.get_stop_after(), Some(102));
    }

    #[test]
    fn test_clear_stop_after_resets_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.set_stop_after(101);

        queue.clear_stop_after();

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_set_stop_after_silently_ignores_unknown_id() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));

        queue.set_stop_after(999); // not in queue

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_set_stop_after_on_empty_queue_is_noop() {
        let queue = QueueManager::new();

        queue.set_stop_after(101);

        assert_eq!(queue.get_stop_after(), None);
    }

    // ============ Stop-After Marker — Consume (Firing Path) ============

    #[test]
    fn test_consume_stop_after_if_fires_on_match() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.set_stop_after(102);

        let fired = queue.consume_stop_after_if(102);

        assert!(fired, "consume should return true on match");
        assert_eq!(queue.get_stop_after(), None, "marker should be cleared after firing");
    }

    #[test]
    fn test_consume_stop_after_if_does_not_fire_on_mismatch() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.set_stop_after(102);

        let fired = queue.consume_stop_after_if(101);

        assert!(!fired, "consume should return false on mismatch");
        assert_eq!(queue.get_stop_after(), Some(102), "marker should remain on mismatch");
    }

    #[test]
    fn test_consume_stop_after_if_with_no_marker_returns_false() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));

        let fired = queue.consume_stop_after_if(101);

        assert!(!fired);
    }

    // ============ Stop-After Marker — Invalidation on Queue Mutations ============

    #[test]
    fn test_set_queue_invalidates_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.set_stop_after(102);

        queue.set_queue(vec![create_test_track(201), create_test_track(202)], None);

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_clear_invalidates_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.set_stop_after(102);

        queue.clear(true);

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_remove_track_invalidates_marker_when_marked_track_removed() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.add_track(create_test_track(103));
        queue.set_stop_after(102);

        queue.remove_track(1); // removes track 102

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_remove_track_keeps_marker_when_other_track_removed() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.add_track(create_test_track(103));
        queue.set_stop_after(102);

        queue.remove_track(0); // removes track 101

        assert_eq!(queue.get_stop_after(), Some(102));
    }

    #[test]
    fn test_move_track_does_not_invalidate_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));
        queue.add_track(create_test_track(103));
        queue.set_stop_after(102);

        queue.move_track(1, 0); // 102 moves to position 0

        assert_eq!(queue.get_stop_after(), Some(102));
    }

    #[test]
    fn test_remove_after_returns_count() {
        let queue = QueueManager::new();
        for id in [101, 102, 103, 104, 105] {
            queue.add_track(create_test_track(id));
        }

        let removed = queue.remove_after(1);

        assert_eq!(removed, 3, "should remove indices 2, 3, 4");
        let state = queue.get_state();
        assert_eq!(state.total_tracks, 2);
    }

    #[test]
    fn test_remove_after_on_last_index_is_noop() {
        let queue = QueueManager::new();
        for id in [101, 102, 103] {
            queue.add_track(create_test_track(id));
        }

        let removed = queue.remove_after(2);

        assert_eq!(removed, 0);
        assert_eq!(queue.get_state().total_tracks, 3);
    }

    #[test]
    fn test_remove_after_with_index_out_of_bounds_is_noop() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.add_track(create_test_track(102));

        let removed = queue.remove_after(99);

        assert_eq!(removed, 0);
        assert_eq!(queue.get_state().total_tracks, 2);
    }

    #[test]
    fn test_remove_after_invalidates_marker_when_in_removed_range() {
        let queue = QueueManager::new();
        for id in [101, 102, 103, 104] {
            queue.add_track(create_test_track(id));
        }
        queue.set_stop_after(103);

        queue.remove_after(1); // removes 103, 104

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_remove_after_keeps_marker_when_before_range() {
        let queue = QueueManager::new();
        for id in [101, 102, 103, 104] {
            queue.add_track(create_test_track(id));
        }
        queue.set_stop_after(101);

        queue.remove_after(2); // removes 104 only (index 3)

        assert_eq!(queue.get_stop_after(), Some(101));
    }

    #[test]
    fn test_remove_after_keeps_marker_when_at_pivot_index() {
        let queue = QueueManager::new();
        for id in [101, 102, 103, 104] {
            queue.add_track(create_test_track(id));
        }
        queue.set_stop_after(102);

        queue.remove_after(1); // removes indices 2, 3 — track 102 (at index 1) stays

        assert_eq!(queue.get_stop_after(), Some(102));
    }

    #[test]
    fn test_remove_upcoming_after_linear() {
        let queue = QueueManager::new();
        // 101 playing, upcoming = [102, 103, 104, 105].
        queue.set_queue((101..=105).map(create_test_track).collect(), Some(0));

        // Keep upcoming positions 0..=1 (102, 103); drop 2, 3 (104, 105).
        let removed = queue.remove_upcoming_after(1);

        assert_eq!(removed, 2);
        let state = queue.get_state_full();
        assert_eq!(state.current_track.map(|t| t.id), Some(101));
        assert_eq!(
            state.upcoming.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![102, 103]
        );
    }

    #[test]
    fn test_remove_upcoming_after_on_last_upcoming_is_noop() {
        let queue = QueueManager::new();
        queue.set_queue((101..=103).map(create_test_track).collect(), Some(0));
        // Upcoming = [102, 103]; position 1 is the last, nothing after it.
        let removed = queue.remove_upcoming_after(1);
        assert_eq!(removed, 0);
        assert_eq!(queue.get_state().total_tracks, 3);
    }

    #[test]
    fn test_remove_upcoming_after_no_current_track() {
        let queue = QueueManager::new();
        // No current index -> every track is upcoming.
        queue.set_queue((101..=104).map(create_test_track).collect(), None);
        let removed = queue.remove_upcoming_after(0);
        assert_eq!(removed, 3);
        let state = queue.get_state_full();
        assert_eq!(
            state.upcoming.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![101]
        );
    }

    #[test]
    fn test_remove_upcoming_after_respects_shuffle_play_order() {
        let queue = QueueManager::new();
        // tracks: 101..105 at indices 0..4. Shuffle play order = [2,0,4,1,3]
        // -> 103(current), then upcoming 101, 105, 102, 104.
        queue.set_queue_with_order(
            (101..=105).map(create_test_track).collect(),
            Some(2),
            true,
            Some(vec![2, 0, 4, 1, 3]),
        );

        // Keep upcoming positions 0..=1 (101, 105); drop 2, 3 (102, 104).
        let removed = queue.remove_upcoming_after(1);

        assert_eq!(removed, 2);
        let state = queue.get_state_full();
        assert_eq!(state.current_track.map(|t| t.id), Some(103));
        assert_eq!(
            state.upcoming.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![101, 105]
        );
    }

    #[test]
    fn test_remove_upcoming_after_invalidates_marker_in_removed_range() {
        let queue = QueueManager::new();
        queue.set_queue((101..=105).map(create_test_track).collect(), Some(0));
        queue.set_stop_after(104); // upcoming position 2 -> removed by after(1)

        queue.remove_upcoming_after(1);

        assert_eq!(queue.get_stop_after(), None);
    }

    #[test]
    fn test_get_state_includes_stop_after() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));
        queue.set_stop_after(101);

        let state = queue.get_state();

        assert_eq!(state.stop_after_track_id, Some(101));
    }

    #[test]
    fn test_get_state_full_returns_uncapped_upcoming() {
        let queue = QueueManager::new();
        // 50 tracks — more than get_state()'s 20-track upcoming cap.
        for i in 1..=50 {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(0);

        let capped = queue.get_state();
        assert_eq!(capped.upcoming.len(), 20, "get_state caps upcoming at 20");

        let full = queue.get_state_full();
        assert_eq!(full.upcoming.len(), 49, "get_state_full returns all upcoming");
        assert_eq!(full.total_tracks, 50);
        assert_eq!(full.upcoming.first().unwrap().id, 2);
        assert_eq!(full.upcoming.last().unwrap().id, 50);
    }

    #[test]
    fn test_get_state_full_returns_uncapped_history() {
        let queue = QueueManager::new();
        for i in 1..=30 {
            queue.add_track(create_test_track(i));
        }
        queue.play_index(0);
        // Advance through 25 tracks — more than get_state()'s 10-entry cap.
        for _ in 0..25 {
            queue.next();
        }

        let capped = queue.get_state();
        assert_eq!(capped.history.len(), 10, "get_state caps history at 10");

        let full = queue.get_state_full();
        assert_eq!(full.history.len(), 25, "get_state_full returns all history");
        // Newest-first ordering: most recently played sits at the front.
        assert_eq!(full.history.first().unwrap().id, 25);
    }

    #[test]
    fn test_persistable_state_restores_bounded_history() {
        let queue = QueueManager::new();
        queue.set_queue((1..=60).map(create_test_track).collect(), Some(55));

        // Oldest-first, as captured on disk. More than the live 50-entry cap.
        queue.restore_history_indices((0..55).collect());

        let (tracks, current, history) = queue.get_persistable_state();
        assert_eq!(tracks.len(), 60);
        assert_eq!(current, Some(55));
        assert_eq!(history.len(), 50);
        assert_eq!(history.first(), Some(&5));
        assert_eq!(history.last(), Some(&54));

        let full = queue.get_state_full();
        assert_eq!(full.history.first().map(|track| track.id), Some(55));
        assert_eq!(full.history.last().map(|track| track.id), Some(6));
    }

    #[test]
    fn test_restore_history_discards_invalid_indices() {
        let queue = QueueManager::new();
        queue.set_queue((1..=3).map(create_test_track).collect(), Some(2));

        queue.restore_history_indices(vec![0, usize::MAX, 1, 99]);

        assert_eq!(queue.get_persistable_state().2, vec![0, 1]);
        assert_eq!(
            queue
                .get_state_full()
                .history
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn test_get_state_full_no_current_track_returns_all_as_upcoming() {
        let queue = QueueManager::new();
        for i in 1..=5 {
            queue.add_track(create_test_track(i));
        }

        let full = queue.get_state_full();
        assert!(full.current_track.is_none());
        assert_eq!(full.upcoming.len(), 5);
    }

    #[test]
    fn test_get_state_returns_none_when_no_marker() {
        let queue = QueueManager::new();
        queue.add_track(create_test_track(101));

        let state = queue.get_state();

        assert_eq!(state.stop_after_track_id, None);
    }
}
