use std::collections::{HashMap, HashSet};

use crate::queue::{QConnectQueueState, QueueEvent, QueueItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReducerOutcome {
    pub queue_changed: bool,
    pub version_changed: bool,
    pub event_name: &'static str,
}

impl ReducerOutcome {
    const fn unchanged(event_name: &'static str) -> Self {
        Self {
            queue_changed: false,
            version_changed: false,
            event_name,
        }
    }

    const fn changed(event_name: &'static str, version_changed: bool) -> Self {
        Self {
            queue_changed: true,
            version_changed,
            event_name,
        }
    }
}

pub fn apply_event(
    state: &mut QConnectQueueState,
    event: &QueueEvent,
    now_ms: u64,
) -> ReducerOutcome {
    match event {
        QueueEvent::QueueStateReplaced { state: next, .. } => {
            *state = next.clone();
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_QUEUE_STATE", true)
        }
        QueueEvent::TracksAdded {
            version,
            tracks,
            shuffle_seed,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            let start_idx = state.queue_items.len();
            state.queue_items.extend(tracks.iter().cloned());

            if state.shuffle_mode {
                let prior_order = if start_idx == 0 {
                    Some(Vec::new())
                } else {
                    state.shuffle_order.take()
                };
                state.shuffle_order = prior_order.zip(*shuffle_seed).map(|(mut order, seed)| {
                    order.extend(
                        build_shuffle_order(tracks.len(), seed, None)
                            .into_iter()
                            .map(|index| start_idx + index),
                    );
                    order
                });
            }

            if *autoplay_reset {
                state.autoplay_items.clear();
            }
            state.autoplay_loading = *autoplay_loading;

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_QUEUE_TRACKS_ADDED", version_changed)
        }
        QueueEvent::TracksLoaded {
            version,
            tracks,
            shuffle_mode,
            shuffle_seed,
            shuffle_pivot_queue_item_id,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            state.queue_items = tracks.clone();

            if let Some(enabled) = shuffle_mode {
                state.shuffle_mode = *enabled;
            }

            state.shuffle_order = if state.shuffle_mode {
                shuffle_seed.map(|seed| {
                    let pivot_index = shuffle_pivot_queue_item_id.and_then(|queue_item_id| {
                        state
                            .queue_items
                            .iter()
                            .position(|item| item.queue_item_id == queue_item_id)
                    });
                    build_shuffle_order(state.queue_items.len(), seed, pivot_index)
                })
            } else {
                None
            };

            if *autoplay_reset {
                state.autoplay_items.clear();
            }
            state.autoplay_loading = *autoplay_loading;

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_QUEUE_TRACKS_LOADED", version_changed)
        }
        QueueEvent::TracksInserted {
            version,
            tracks,
            insert_after,
            shuffle_seed,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            let before = state.queue_items.clone();
            let insert_index = insertion_index(&state.queue_items, *insert_after);
            let insert_count = tracks.len();
            let mut next_queue =
                Vec::with_capacity(state.queue_items.len().saturating_add(insert_count));
            next_queue.extend_from_slice(&state.queue_items[..insert_index]);
            next_queue.extend(tracks.iter().cloned());
            next_queue.extend_from_slice(&state.queue_items[insert_index..]);
            state.queue_items = next_queue;

            if state.shuffle_mode {
                let prior_order = if before.is_empty() {
                    Some(Vec::new())
                } else {
                    state.shuffle_order.take()
                };
                state.shuffle_order = prior_order.zip(*shuffle_seed).map(|(mut order, seed)| {
                    let insertion_position = insert_after
                        .and_then(|queue_item_id| {
                            before
                                .iter()
                                .position(|item| item.queue_item_id == queue_item_id)
                        })
                        .and_then(|old_index| order.iter().position(|index| *index == old_index))
                        .map(|position| position + 1)
                        .unwrap_or(0);

                    for index in &mut order {
                        if *index >= insert_index {
                            *index += insert_count;
                        }
                    }
                    let added = build_shuffle_order(insert_count, seed, None)
                        .into_iter()
                        .map(|index| insert_index + index);
                    order.splice(insertion_position..insertion_position, added);
                    order
                });
            }

            if *autoplay_reset {
                state.autoplay_items.clear();
            }
            state.autoplay_loading = *autoplay_loading;

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_QUEUE_TRACKS_INSERTED", version_changed)
        }
        QueueEvent::TracksRemoved {
            version,
            queue_item_ids,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            let before = state.queue_items.clone();
            let to_remove: HashSet<u64> = queue_item_ids.iter().copied().collect();
            let old_len = before.len();

            let mut old_to_new = vec![None; old_len];
            let mut next_queue = Vec::with_capacity(old_len);
            for (old_idx, item) in before.iter().cloned().enumerate() {
                if !to_remove.contains(&item.queue_item_id) {
                    old_to_new[old_idx] = Some(next_queue.len());
                    next_queue.push(item);
                }
            }

            let removed = old_len != next_queue.len();
            state.queue_items = next_queue;

            if let Some(order) = state.shuffle_order.as_mut() {
                let mapped: Vec<usize> = order
                    .iter()
                    .filter_map(|old_idx| old_to_new.get(*old_idx).and_then(|value| *value))
                    .collect();
                *order = mapped;
            }

            if *autoplay_reset {
                state.autoplay_items.clear();
            }
            state.autoplay_loading = *autoplay_loading;

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;

            if removed {
                ReducerOutcome::changed("SRVR_CTRL_QUEUE_TRACKS_REMOVED", version_changed)
            } else {
                ReducerOutcome::unchanged("SRVR_CTRL_QUEUE_TRACKS_REMOVED")
            }
        }
        QueueEvent::TracksReordered {
            version,
            queue_item_ids,
            insert_after,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            let before = state.queue_items.clone();
            let to_move: HashSet<u64> = queue_item_ids.iter().copied().collect();
            let mut by_id: HashMap<u64, QueueItem> = before
                .iter()
                .cloned()
                .map(|item| (item.queue_item_id, item))
                .collect();

            let moving: Vec<QueueItem> = queue_item_ids
                .iter()
                .filter_map(|id| by_id.remove(id))
                .collect();
            let mut remaining: Vec<QueueItem> = before
                .iter()
                .filter(|item| !to_move.contains(&item.queue_item_id))
                .cloned()
                .collect();
            let insert_index = insertion_index(&remaining, *insert_after);

            let reordered = !moving.is_empty();
            if reordered {
                remaining.splice(insert_index..insert_index, moving);
                state.queue_items = remaining;

                if let Some(order) = state.shuffle_order.as_mut() {
                    let id_by_old_index: HashMap<usize, u64> = before
                        .iter()
                        .enumerate()
                        .map(|(idx, item)| (idx, item.queue_item_id))
                        .collect();
                    let new_index_by_id: HashMap<u64, usize> = state
                        .queue_items
                        .iter()
                        .enumerate()
                        .map(|(idx, item)| (item.queue_item_id, idx))
                        .collect();

                    let mapped: Vec<usize> = order
                        .iter()
                        .filter_map(|old_idx| id_by_old_index.get(old_idx))
                        .filter_map(|id| new_index_by_id.get(id))
                        .copied()
                        .collect();
                    *order = mapped;
                }
            }

            if *autoplay_reset {
                state.autoplay_items.clear();
            }
            state.autoplay_loading = *autoplay_loading;

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;

            if reordered {
                ReducerOutcome::changed("SRVR_CTRL_QUEUE_TRACKS_REORDERED", version_changed)
            } else {
                ReducerOutcome::unchanged("SRVR_CTRL_QUEUE_TRACKS_REORDERED")
            }
        }
        QueueEvent::QueueCleared { version, .. } => {
            state.queue_items.clear();
            state.autoplay_items.clear();
            state.shuffle_mode = false;
            state.shuffle_order = None;
            state.autoplay_loading = false;
            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_QUEUE_CLEARED", version_changed)
        }
        QueueEvent::ShuffleModeSet {
            version,
            shuffle_mode,
            shuffle_seed,
            shuffle_pivot_queue_item_id,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            state.shuffle_mode = *shuffle_mode;
            state.shuffle_order = if *shuffle_mode {
                shuffle_seed.map(|seed| {
                    let pivot_index = shuffle_pivot_queue_item_id.and_then(|queue_item_id| {
                        state
                            .queue_items
                            .iter()
                            .position(|item| item.queue_item_id == queue_item_id)
                    });
                    build_shuffle_order(state.queue_items.len(), seed, pivot_index)
                })
            } else {
                None
            };

            if *autoplay_reset {
                state.autoplay_items.clear();
            }
            state.autoplay_loading = *autoplay_loading;

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_SHUFFLE_MODE_SET", version_changed)
        }
        QueueEvent::AutoplayModeSet {
            version,
            autoplay_mode,
            autoplay_reset,
            autoplay_loading,
            ..
        } => {
            state.autoplay_mode = *autoplay_mode;
            state.autoplay_loading = *autoplay_loading;
            if *autoplay_reset {
                state.autoplay_items.clear();
            }

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_AUTOPLAY_MODE_SET", version_changed)
        }
        QueueEvent::AutoplayTracksLoaded {
            version, tracks, ..
        } => {
            state.autoplay_items = tracks.clone();
            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;
            ReducerOutcome::changed("SRVR_CTRL_AUTOPLAY_TRACKS_LOADED", version_changed)
        }
        QueueEvent::AutoplayTracksRemoved {
            version,
            queue_item_ids,
            ..
        } => {
            let to_remove: HashSet<u64> = queue_item_ids.iter().copied().collect();
            let before = state.autoplay_items.len();
            state
                .autoplay_items
                .retain(|track| !to_remove.contains(&track.queue_item_id));
            let changed = before != state.autoplay_items.len();

            let version_changed = state.version != *version;
            state.version = *version;
            state.updated_at_ms = now_ms;

            if changed {
                ReducerOutcome::changed("SRVR_CTRL_AUTOPLAY_TRACKS_REMOVED", version_changed)
            } else {
                ReducerOutcome::unchanged("SRVR_CTRL_AUTOPLAY_TRACKS_REMOVED")
            }
        }
        QueueEvent::QueueError { version, .. } => {
            if let Some(version) = version {
                let version_changed = state.version != *version;
                state.version = *version;
                state.updated_at_ms = now_ms;
                return ReducerOutcome {
                    queue_changed: false,
                    version_changed,
                    event_name: "SRVR_CTRL_QUEUE_ERROR_MESSAGE",
                };
            }
            ReducerOutcome::unchanged("SRVR_CTRL_QUEUE_ERROR_MESSAGE")
        }
    }
}

fn insertion_index(items: &[QueueItem], insert_after: Option<u64>) -> usize {
    insert_after
        .and_then(|queue_item_id| {
            items
                .iter()
                .position(|item| item.queue_item_id == queue_item_id)
                .map(|idx| idx + 1)
        })
        .unwrap_or(0)
}

/// Reproduce the official clients' QConnect shuffle exactly.
///
/// The Web Player consumes the server-provided fixed32 seed with xoshiro128**
/// and Fisher-Yates. When a pivot is present it is removed before shuffling and
/// prepended afterwards. This function must never substitute local entropy:
/// equal WS seed + queue + pivot must yield the same order on every renderer.
pub fn build_shuffle_order(count: usize, seed: u64, pivot_index: Option<usize>) -> Vec<usize> {
    let pivot = pivot_index.filter(|value| *value < count);
    let mut order: Vec<usize> = if let Some(pivot) = pivot {
        (0..count).filter(|index| *index != pivot).collect()
    } else {
        (0..count).collect()
    };
    let mut rng = Xoshiro128StarStar::from_seed(seed as u32);

    for idx in (1..order.len()).rev() {
        let swap_idx = (rng.next() % ((idx + 1) as u32)) as usize;
        order.swap(idx, swap_idx);
    }

    if let Some(pivot) = pivot {
        order.insert(0, pivot);
    }

    order
}

struct Xoshiro128StarStar {
    state: [u32; 4],
}

impl Xoshiro128StarStar {
    fn from_seed(seed: u32) -> Self {
        let mut splitmix_state = seed;
        let mut next_splitmix = || {
            splitmix_state = splitmix_state.wrapping_add(0x9e37_79b9);
            let mut value = splitmix_state;
            value = (value ^ (value >> 16)).wrapping_mul(0x85eb_ca6b);
            value = (value ^ (value >> 13)).wrapping_mul(0xc2b2_ae35);
            value ^ (value >> 16)
        };
        Self {
            state: [
                next_splitmix(),
                next_splitmix(),
                next_splitmix(),
                next_splitmix(),
            ],
        }
    }

    fn next(&mut self) -> u32 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let shifted = self.state[1] << 9;

        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= shifted;
        self.state[3] = self.state[3].rotate_left(11);

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{QueueEvent, QueueVersion};

    fn item(id: u64) -> QueueItem {
        QueueItem {
            track_context_uuid: "ctx".to_string(),
            track_id: id,
            queue_item_id: id,
        }
    }

    #[test]
    fn tracks_loaded_reproduces_server_seed_and_pivot() {
        let mut state = QConnectQueueState::default();
        let event = QueueEvent::TracksLoaded {
            action_uuid: None,
            version: QueueVersion::new(1, 1),
            tracks: vec![item(10), item(20), item(30)],
            queue_position: Some(1),
            shuffle_mode: Some(true),
            shuffle_seed: Some(42),
            shuffle_pivot_queue_item_id: Some(20),
            autoplay_reset: true,
            autoplay_loading: true,
        };

        let outcome = apply_event(&mut state, &event, 1234);
        assert!(outcome.queue_changed);
        assert_eq!(state.queue_items.len(), 3);
        assert!(state.shuffle_mode);
        assert_eq!(state.shuffle_order, Some(vec![1, 2, 0]));
        assert!(state.autoplay_loading);
    }

    #[test]
    fn shuffle_mode_set_replaces_stale_order_from_server_seed() {
        let mut state = QConnectQueueState {
            queue_items: vec![item(1), item(2), item(3)],
            shuffle_mode: false,
            shuffle_order: Some(vec![2, 1, 0]),
            ..Default::default()
        };

        let event = QueueEvent::ShuffleModeSet {
            action_uuid: None,
            version: QueueVersion::new(1, 2),
            shuffle_mode: true,
            shuffle_seed: Some(42),
            shuffle_pivot_queue_item_id: Some(1),
            autoplay_reset: false,
            autoplay_loading: false,
        };

        let outcome = apply_event(&mut state, &event, 2345);
        assert!(outcome.queue_changed);
        assert!(state.shuffle_mode);
        assert_eq!(state.shuffle_order, Some(vec![0, 2, 1]));
    }

    #[test]
    fn official_web_player_xoshiro_fixtures_match_exactly() {
        assert_eq!(
            build_shuffle_order(10, 0, None),
            vec![3, 6, 1, 5, 0, 4, 2, 9, 7, 8]
        );
        assert_eq!(
            build_shuffle_order(10, 42, None),
            vec![3, 6, 8, 7, 5, 0, 9, 2, 1, 4]
        );
        assert_eq!(
            build_shuffle_order(10, 42, Some(3)),
            vec![3, 4, 5, 8, 2, 9, 0, 6, 1, 7]
        );
    }

    #[test]
    fn shuffled_add_uses_only_the_ws_seed_for_the_new_batch() {
        let mut state = QConnectQueueState {
            queue_items: vec![item(1), item(2), item(3)],
            shuffle_mode: true,
            shuffle_order: Some(vec![2, 0, 1]),
            ..Default::default()
        };
        let event = QueueEvent::TracksAdded {
            action_uuid: None,
            version: QueueVersion::new(2, 0),
            tracks: vec![item(8), item(9)],
            shuffle_seed: Some(42),
            autoplay_reset: false,
            autoplay_loading: false,
        };

        apply_event(&mut state, &event, 1);

        assert_eq!(state.shuffle_order, Some(vec![2, 0, 1, 4, 3]));
    }

    #[test]
    fn shuffled_insert_matches_official_original_and_playback_positions() {
        let mut state = QConnectQueueState {
            queue_items: vec![item(1), item(2), item(3)],
            shuffle_mode: true,
            shuffle_order: Some(vec![2, 0, 1]),
            ..Default::default()
        };
        let event = QueueEvent::TracksInserted {
            action_uuid: None,
            version: QueueVersion::new(2, 0),
            tracks: vec![item(8), item(9)],
            insert_after: Some(1),
            shuffle_seed: Some(42),
            autoplay_reset: false,
            autoplay_loading: false,
        };

        apply_event(&mut state, &event, 1);

        assert_eq!(
            state
                .queue_items
                .iter()
                .map(|entry| entry.queue_item_id)
                .collect::<Vec<_>>(),
            vec![1, 8, 9, 2, 3]
        );
        assert_eq!(state.shuffle_order, Some(vec![4, 0, 2, 1, 3]));
    }

    #[test]
    fn insert_without_anchor_goes_to_the_front() {
        let mut state = QConnectQueueState {
            queue_items: vec![item(1), item(2)],
            ..Default::default()
        };
        let event = QueueEvent::TracksInserted {
            action_uuid: None,
            version: QueueVersion::new(2, 0),
            tracks: vec![item(9)],
            insert_after: None,
            shuffle_seed: None,
            autoplay_reset: false,
            autoplay_loading: false,
        };

        apply_event(&mut state, &event, 1);

        assert_eq!(
            state
                .queue_items
                .iter()
                .map(|entry| entry.queue_item_id)
                .collect::<Vec<_>>(),
            vec![9, 1, 2]
        );
    }

    #[test]
    fn tracks_removed_updates_queue_and_shuffle() {
        let mut state = QConnectQueueState {
            queue_items: vec![item(1), item(2), item(3)],
            shuffle_mode: true,
            shuffle_order: Some(vec![2, 1, 0]),
            ..Default::default()
        };

        let event = QueueEvent::TracksRemoved {
            action_uuid: None,
            version: QueueVersion::new(2, 0),
            queue_item_ids: vec![2],
            autoplay_reset: false,
            autoplay_loading: false,
        };

        let outcome = apply_event(&mut state, &event, 22);
        assert!(outcome.queue_changed);
        assert_eq!(state.queue_items.len(), 2);
        assert_eq!(state.queue_items[0].queue_item_id, 1);
        assert_eq!(state.queue_items[1].queue_item_id, 3);
        assert_eq!(state.shuffle_order, Some(vec![1, 0]));
    }

    #[test]
    fn tracks_reordered_moves_subset() {
        let mut state = QConnectQueueState {
            queue_items: vec![item(1), item(2), item(3), item(4)],
            shuffle_mode: true,
            shuffle_order: Some(vec![0, 1, 2, 3]),
            ..Default::default()
        };

        let event = QueueEvent::TracksReordered {
            action_uuid: None,
            version: QueueVersion::new(3, 0),
            queue_item_ids: vec![4],
            insert_after: Some(1),
            autoplay_reset: false,
            autoplay_loading: false,
        };

        let outcome = apply_event(&mut state, &event, 33);
        assert!(outcome.queue_changed);
        assert_eq!(
            state
                .queue_items
                .iter()
                .map(|entry| entry.queue_item_id)
                .collect::<Vec<_>>(),
            vec![1, 4, 2, 3]
        );
    }
}
