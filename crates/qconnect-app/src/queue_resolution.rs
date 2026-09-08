//! Queue cursor abstractions and index/order resolution helpers used to
//! map between cloud-side queue/renderer state and local queue indices.
//!
//! The QConnect protocol mixes queue_item_id, track_id, and ordering
//! data across multiple separate frames; this module owns the lookups
//! that reconcile those signals into a single coherent cursor.

use crate::{QConnectQueueState, QConnectRendererState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QconnectOrderedQueueCursor {
    Queue(usize),
    Autoplay(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QconnectRemoteSkipDirection {
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QconnectControllerQueueItemResolution {
    pub target_queue_item_id: Option<u64>,
    pub strategy: &'static str,
    pub queue_index: Option<usize>,
    pub matched_track_id: Option<u64>,
    pub matched_queue_item_id: Option<u64>,
}

pub fn is_valid_ordered_queue_shuffle_order(order: &[usize], track_count: usize) -> bool {
    if order.len() != track_count {
        return false;
    }
    let mut seen = vec![false; track_count];
    for &index in order {
        if index >= track_count || seen[index] {
            return false;
        }
        seen[index] = true;
    }
    true
}

pub fn ordered_queue_cursors(queue: &QConnectQueueState) -> Vec<QconnectOrderedQueueCursor> {
    let mut cursors = if queue.shuffle_mode {
        queue
            .shuffle_order
            .as_ref()
            .filter(|order| is_valid_ordered_queue_shuffle_order(order, queue.queue_items.len()))
            .map(|order| {
                order
                    .iter()
                    .copied()
                    .map(QconnectOrderedQueueCursor::Queue)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                queue
                    .queue_items
                    .iter()
                    .enumerate()
                    .map(|(index, _)| QconnectOrderedQueueCursor::Queue(index))
                    .collect::<Vec<_>>()
            })
    } else {
        queue
            .queue_items
            .iter()
            .enumerate()
            .map(|(index, _)| QconnectOrderedQueueCursor::Queue(index))
            .collect::<Vec<_>>()
    };

    cursors.extend(
        queue
            .autoplay_items
            .iter()
            .enumerate()
            .map(|(index, _)| QconnectOrderedQueueCursor::Autoplay(index)),
    );
    cursors
}

pub fn queue_item_track_id_for_cursor(
    queue: &QConnectQueueState,
    cursor: QconnectOrderedQueueCursor,
) -> Option<u64> {
    match cursor {
        QconnectOrderedQueueCursor::Queue(index) => {
            queue.queue_items.get(index).map(|item| item.track_id)
        }
        QconnectOrderedQueueCursor::Autoplay(index) => {
            queue.autoplay_items.get(index).map(|item| item.track_id)
        }
    }
}

pub fn normalized_queue_item_id_for_cursor(
    queue: &QConnectQueueState,
    cursor: QconnectOrderedQueueCursor,
) -> Option<u64> {
    match cursor {
        QconnectOrderedQueueCursor::Queue(index) => Some(
            normalize_current_queue_item_id_from_queue_state(queue, index),
        ),
        QconnectOrderedQueueCursor::Autoplay(index) => queue
            .autoplay_items
            .get(index)
            .map(|item| item.queue_item_id),
    }
}

pub fn find_cursor_index_by_queue_item_id(
    cursors: &[QconnectOrderedQueueCursor],
    queue: &QConnectQueueState,
    queue_item_id: Option<u64>,
) -> Option<usize> {
    let queue_item_id = queue_item_id?;
    cursors.iter().position(|cursor| {
        normalized_queue_item_id_for_cursor(queue, *cursor) == Some(queue_item_id)
            || match cursor {
                QconnectOrderedQueueCursor::Queue(index) => queue
                    .queue_items
                    .get(*index)
                    .map(|item| item.queue_item_id == queue_item_id)
                    .unwrap_or(false),
                QconnectOrderedQueueCursor::Autoplay(index) => queue
                    .autoplay_items
                    .get(*index)
                    .map(|item| item.queue_item_id == queue_item_id)
                    .unwrap_or(false),
            }
    })
}

pub fn find_cursor_index_by_track_id(
    cursors: &[QconnectOrderedQueueCursor],
    queue: &QConnectQueueState,
    track_id: Option<u64>,
) -> Option<usize> {
    let track_id = track_id?;
    cursors
        .iter()
        .position(|cursor| queue_item_track_id_for_cursor(queue, *cursor) == Some(track_id))
}

fn find_cursor_index_by_track_id_before(
    cursors: &[QconnectOrderedQueueCursor],
    queue: &QConnectQueueState,
    track_id: Option<u64>,
    end_exclusive: usize,
) -> Option<usize> {
    let track_id = track_id?;
    if end_exclusive == 0 {
        return None;
    }

    for index in (0..end_exclusive).rev() {
        if queue_item_track_id_for_cursor(queue, cursors[index]) == Some(track_id) {
            return Some(index);
        }
    }

    None
}

fn resolve_current_cursor_index_from_snapshots(
    queue: &QConnectQueueState,
    renderer: &QConnectRendererState,
    cursors: &[QconnectOrderedQueueCursor],
) -> (Option<usize>, &'static str) {
    let current_queue_index = find_cursor_index_by_queue_item_id(
        cursors,
        queue,
        renderer
            .current_track
            .as_ref()
            .map(|item| item.queue_item_id),
    );
    if current_queue_index.is_some() {
        return (
            current_queue_index,
            "renderer_current_queue_item_id_verified",
        );
    }

    let next_queue_index = find_cursor_index_by_queue_item_id(
        cursors,
        queue,
        renderer.next_track.as_ref().map(|item| item.queue_item_id),
    );
    let track_index_before_next = next_queue_index.and_then(|next_index| {
        find_cursor_index_by_track_id_before(
            cursors,
            queue,
            renderer.current_track.as_ref().map(|item| item.track_id),
            next_index,
        )
    });
    if track_index_before_next.is_some() {
        return (
            track_index_before_next,
            "queue_track_id_before_renderer_next",
        );
    }

    let current_track_index = find_cursor_index_by_track_id(
        cursors,
        queue,
        renderer.current_track.as_ref().map(|item| item.track_id),
    );
    if current_track_index.is_some() {
        return (current_track_index, "queue_track_id_match");
    }

    // A prefetched successor does not identify the current item. In repeat-one
    // it can name the current item itself; without a current qid/track match,
    // inferring its predecessor would invent a cursor and send a wrong skip.
    (None, "no_current_queue_item")
}

pub fn resolve_controller_queue_item_from_snapshots(
    queue: &QConnectQueueState,
    renderer: &QConnectRendererState,
    direction: QconnectRemoteSkipDirection,
) -> QconnectControllerQueueItemResolution {
    // Do not fabricate a linear order while a shuffled queue is waiting for
    // its WS-authored indexes. No local reshuffle or next-track hint is an
    // authoritative substitute.
    if queue.shuffle_mode
        && !queue.queue_items.is_empty()
        && !queue.shuffle_order.as_ref().is_some_and(|order| {
            is_valid_ordered_queue_shuffle_order(order, queue.queue_items.len())
        })
    {
        return QconnectControllerQueueItemResolution {
            target_queue_item_id: None,
            strategy: "missing_authoritative_shuffle_order",
            queue_index: None,
            matched_track_id: None,
            matched_queue_item_id: None,
        };
    }

    let cursors = ordered_queue_cursors(queue);
    if cursors.is_empty() {
        return QconnectControllerQueueItemResolution {
            target_queue_item_id: None,
            strategy: "no_queue_items",
            queue_index: None,
            matched_track_id: None,
            matched_queue_item_id: None,
        };
    }

    let (current_index, _current_strategy) =
        resolve_current_cursor_index_from_snapshots(queue, renderer, &cursors);

    let Some(current_index) = current_index else {
        return QconnectControllerQueueItemResolution {
            target_queue_item_id: None,
            strategy: "no_current_queue_item",
            queue_index: None,
            matched_track_id: None,
            matched_queue_item_id: None,
        };
    };

    // Official Mac 8.2 getNextIndexOnUserAction/getPreviousIndex: manual
    // Next ignores repeat-one (unlike the prefetch/completion successor),
    // repeat-all wraps ONLY the main queue, and off/one clamp at the boundary.
    // The combined cursor sequence already preserves the WS shuffle order.
    let repeat_all = renderer.loop_mode == Some(3);
    let main_len = queue.queue_items.len();
    if repeat_all && main_len == 0 {
        return QconnectControllerQueueItemResolution {
            target_queue_item_id: None,
            strategy: "no_repeat_queue_items",
            queue_index: None,
            matched_track_id: None,
            matched_queue_item_id: None,
        };
    }
    let (target_index, strategy) = match direction {
        QconnectRemoteSkipDirection::Next => {
            let target = if repeat_all {
                (current_index + 1) % main_len
            } else {
                (current_index + 1).min(cursors.len() - 1)
            };
            let strategy = if target < current_index {
                "queue_wrap_next"
            } else if target == current_index {
                "last_queue_item"
            } else {
                "queue_item_after_current"
            };
            (target, strategy)
        }
        QconnectRemoteSkipDirection::Previous => {
            let offset = usize::from(renderer.current_position_ms.unwrap_or(0) <= 4000);
            let target = if repeat_all {
                // Reduce before subtracting to avoid unsigned underflow on
                // the first item, including the one-item repeat-all queue.
                (current_index % main_len + main_len - offset % main_len) % main_len
            } else {
                current_index.saturating_sub(offset)
            };
            let strategy = if target == current_index {
                "restart_current_queue_item"
            } else if target > current_index {
                "queue_wrap_previous"
            } else {
                "queue_item_before_current"
            };
            (target, strategy)
        }
    };

    let cursor = cursors[target_index];
    let matched_track_id = queue_item_track_id_for_cursor(queue, cursor);
    let matched_queue_item_id = normalized_queue_item_id_for_cursor(queue, cursor);

    QconnectControllerQueueItemResolution {
        target_queue_item_id: matched_queue_item_id,
        strategy,
        queue_index: Some(target_index),
        matched_track_id,
        matched_queue_item_id,
    }
}

pub fn resolve_queue_item_ids_from_queue_state(
    queue: &QConnectQueueState,
    track_id: u64,
) -> (Option<u64>, Option<u64>, Option<u64>) {
    if let Some(current_index) = queue
        .queue_items
        .iter()
        .position(|item| item.track_id == track_id)
    {
        let current_qid = normalize_current_queue_item_id_from_queue_state(queue, current_index);
        let next_item = if queue.shuffle_mode {
            queue
                .shuffle_order
                .as_ref()
                .and_then(|order| {
                    order
                        .iter()
                        .position(|queue_index| *queue_index == current_index)
                        .and_then(|order_index| order.get(order_index + 1))
                        .and_then(|queue_index| queue.queue_items.get(*queue_index))
                })
                .or_else(|| queue.queue_items.get(current_index + 1))
                .or_else(|| queue.autoplay_items.first())
        } else {
            queue
                .queue_items
                .get(current_index + 1)
                .or_else(|| queue.autoplay_items.first())
        };

        return (
            Some(current_qid),
            next_item.map(|item| item.queue_item_id),
            next_item.map(|item| item.track_id),
        );
    }

    if let Some(current_index) = queue
        .autoplay_items
        .iter()
        .position(|item| item.track_id == track_id)
    {
        let current_item = &queue.autoplay_items[current_index];
        let next_item = queue.autoplay_items.get(current_index + 1);
        return (
            Some(current_item.queue_item_id),
            next_item.map(|item| item.queue_item_id),
            next_item.map(|item| item.track_id),
        );
    }

    (None, None, None)
}

pub fn dedupe_track_ids(queue_state: &QConnectQueueState) -> Vec<u64> {
    let mut unique = Vec::with_capacity(queue_state.queue_items.len());
    for item in &queue_state.queue_items {
        if !unique.contains(&item.track_id) {
            unique.push(item.track_id);
        }
    }
    unique
}

pub fn resolve_remote_start_index(
    queue_state: &QConnectQueueState,
    renderer_queue_item_id: Option<u64>,
    renderer_track_id: Option<u64>,
) -> Option<usize> {
    if let Some(queue_item_id) = renderer_queue_item_id {
        if let Some(index) = queue_state
            .queue_items
            .iter()
            .position(|item| item.queue_item_id == queue_item_id)
        {
            // Only trust the queue_item_id when the track at that position matches
            // the renderer's reported track (or no track was reported). A qid that
            // resolves to a DIFFERENT track means the cached renderer projection is
            // STALE relative to THIS queue — e.g. a fresh album was just pushed (new
            // track_context_uuid, autoplay_reset) while the projection still names
            // the PREVIOUS queue's item. Trusting the stale qid lands the cursor on
            // the wrong track (the "NowPlayingBar shows track 4 on a freshly-pushed
            // album" bug, controlling a peer that was already rendering). Fall
            // through to the track_id lookup; it won't find the old track in the new
            // queue, so the caller defaults to the queue head.
            let track_matches = renderer_track_id
                .map(|track_id| queue_state.queue_items[index].track_id == track_id)
                .unwrap_or(true);
            if track_matches {
                return Some(index);
            }
        }
    }

    if let Some(track_id) = renderer_track_id {
        if let Some(index) = queue_state
            .queue_items
            .iter()
            .position(|item| item.track_id == track_id)
        {
            return Some(index);
        }
    }

    None
}

/// Resolve the `shuffle_pivot_queue_item_id` for an outbound
/// `CtrlSrvrSetShuffleMode` command: the queue item the cloud keeps fixed while
/// it generates the shuffled order (so the currently-playing track stays at the
/// front). Prefers the renderer's reported `queue_item_id`; falls back to the
/// item carrying the renderer's `track_id` when the qid is a placeholder; `None`
/// when the renderer has no current track. Frontend-agnostic (ADR-006): used by
/// both the Tauri and Slint controller shuffle paths.
pub fn resolve_qconnect_shuffle_pivot(
    queue: &QConnectQueueState,
    renderer: &QConnectRendererState,
) -> Option<u64> {
    let current_track = renderer.current_track.as_ref()?;

    if queue
        .queue_items
        .iter()
        .any(|item| item.queue_item_id == current_track.queue_item_id)
    {
        return Some(current_track.queue_item_id);
    }

    if let Some(item) = queue
        .queue_items
        .iter()
        .find(|item| item.track_id == current_track.track_id)
    {
        return Some(item.queue_item_id);
    }

    None
}

/// Return the WS-authored playback order without reinterpretation.
///
/// `shuffled_track_indexes` already is the official session order over the
/// original queue. Current/next belong to the renderer cursor and must never be
/// prepended here: doing so creates a QBZ-only permutation that other official
/// clients cannot observe.
pub fn resolve_core_shuffle_order(queue_state: &QConnectQueueState) -> Option<Vec<usize>> {
    if !queue_state.shuffle_mode {
        return None;
    }

    let raw_order = queue_state
        .shuffle_order
        .as_ref()
        .filter(|order| is_valid_ordered_queue_shuffle_order(order, queue_state.queue_items.len()));

    if raw_order.is_none() {
        log::debug!(
            "[QConnect] resolve_core_shuffle_order: raw_order invalid or absent, items={} order={:?}",
            queue_state.queue_items.len(),
            queue_state.shuffle_order,
        );
        return None;
    }
    Some(raw_order.unwrap().clone())
}

fn is_cloud_placeholder_current_queue_item(
    queue: &QConnectQueueState,
    current_index: usize,
) -> bool {
    let Some(current_item) = queue.queue_items.get(current_index) else {
        return false;
    };

    current_index == 0
        && current_item.queue_item_id == current_item.track_id
        && queue
            .queue_items
            .iter()
            .skip(1)
            .any(|item| item.queue_item_id < current_item.queue_item_id)
}

pub fn normalize_current_queue_item_id_from_queue_state(
    queue: &QConnectQueueState,
    current_index: usize,
) -> u64 {
    if is_cloud_placeholder_current_queue_item(queue, current_index) {
        0
    } else {
        queue.queue_items[current_index].queue_item_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qconnect_core::QueueItem;

    fn item(queue_item_id: u64, track_id: u64) -> QueueItem {
        QueueItem {
            track_context_uuid: String::new(),
            track_id,
            queue_item_id,
        }
    }

    fn queue(items: Vec<QueueItem>) -> QConnectQueueState {
        QConnectQueueState {
            queue_items: items,
            ..Default::default()
        }
    }

    /// Regression (controller of a peer that was already rendering): a fresh album
    /// is pushed, but the cached renderer projection still names the PREVIOUS
    /// queue's item (qid=3 + the old track_id). The old track is absent from the
    /// new queue, so the stale qid must NOT resolve the cursor to the new queue's
    /// item 3 ("NowPlayingBar shows track 4 on a freshly-pushed album"). It falls
    /// through to the track_id lookup (miss) → None → caller defaults to the head.
    #[test]
    fn stale_qid_with_mismatched_track_does_not_land_on_wrong_track() {
        let q = queue(vec![
            item(0, 52848233),
            item(1, 52848234),
            item(2, 52848235),
            item(3, 52848236),
        ]);
        assert_eq!(
            resolve_remote_start_index(&q, Some(3), Some(126886856)),
            None
        );
    }

    /// A consistent projection (qid + the matching track at that position) still
    /// resolves to its index — the normal track-change / takeback path is unchanged.
    #[test]
    fn consistent_qid_and_track_resolves_to_index() {
        let q = queue(vec![item(0, 100), item(1, 200), item(2, 300)]);
        assert_eq!(resolve_remote_start_index(&q, Some(2), Some(300)), Some(2));
    }

    /// When no track is reported, the qid is trusted as before (some events carry
    /// only a queue_item_id).
    #[test]
    fn qid_without_track_id_is_trusted() {
        let q = queue(vec![item(0, 100), item(1, 200)]);
        assert_eq!(resolve_remote_start_index(&q, Some(1), None), Some(1));
    }

    /// An absent qid falls through to the track_id lookup.
    #[test]
    fn track_id_lookup_when_qid_absent_from_queue() {
        let q = queue(vec![item(0, 100), item(7, 200)]);
        assert_eq!(resolve_remote_start_index(&q, Some(99), Some(200)), Some(1));
    }

    #[test]
    fn core_shuffle_projection_preserves_ws_indexes_byte_for_byte() {
        let mut q = queue(vec![
            item(0, 100),
            item(1, 101),
            item(2, 102),
            item(3, 103),
            item(4, 104),
        ]);
        q.shuffle_mode = true;
        q.shuffle_order = Some(vec![4, 2, 0, 3, 1]);

        assert_eq!(resolve_core_shuffle_order(&q), Some(vec![4, 2, 0, 3, 1]));
    }

    /// Shuffle pivot prefers the renderer's reported queue_item_id.
    #[test]
    fn shuffle_pivot_from_renderer_queue_item_id() {
        let q = queue(vec![item(10, 100), item(11, 101), item(12, 102)]);
        let renderer = QConnectRendererState {
            current_track: Some(item(11, 101)),
            ..Default::default()
        };
        assert_eq!(resolve_qconnect_shuffle_pivot(&q, &renderer), Some(11));
    }

    /// When the renderer's qid is a placeholder (0), fall back to the item that
    /// carries the renderer's track_id.
    #[test]
    fn shuffle_pivot_by_track_id_when_qid_is_placeholder() {
        let q = queue(vec![item(20, 200), item(21, 201), item(22, 202)]);
        let renderer = QConnectRendererState {
            current_track: Some(item(0, 202)),
            ..Default::default()
        };
        assert_eq!(resolve_qconnect_shuffle_pivot(&q, &renderer), Some(22));
    }

    #[test]
    fn manual_skip_matches_official_loop_position_shuffle_and_autoplay_matrix() {
        for shuffle in [false, true] {
            for autoplay in [false, true] {
                let mut q = queue(vec![item(0, 100), item(1, 101), item(2, 102)]);
                q.shuffle_mode = shuffle;
                q.shuffle_order = shuffle.then(|| vec![2, 0, 1]);
                q.autoplay_mode = autoplay;
                if autoplay {
                    q.autoplay_items = vec![item(3, 103), item(4, 104)];
                }
                let expected_qids: &[u64] = match (shuffle, autoplay) {
                    (false, false) => &[0, 1, 2],
                    (true, false) => &[2, 0, 1],
                    (false, true) => &[0, 1, 2, 3, 4],
                    (true, true) => &[2, 0, 1, 3, 4],
                };
                for loop_mode in [1, 2, 3] {
                    // Expected indexes are literal fixtures for the official
                    // helpers, not a second copy of the production formula.
                    let next_indexes: &[usize] = match (loop_mode, autoplay) {
                        (3, false) => &[1, 2, 0],
                        (3, true) => &[1, 2, 0, 1, 2],
                        (_, false) => &[1, 2, 2],
                        (_, true) => &[1, 2, 3, 4, 4],
                    };
                    for position_ms in [0, 4000, 4001, 60_000] {
                        let previous_indexes: &[usize] =
                            match (loop_mode, autoplay, position_ms > 4000) {
                                (3, false, false) => &[2, 0, 1],
                                (3, true, false) => &[2, 0, 1, 2, 0],
                                (3, false, true) => &[0, 1, 2],
                                (3, true, true) => &[0, 1, 2, 0, 1],
                                (_, false, false) => &[0, 0, 1],
                                (_, true, false) => &[0, 0, 1, 2, 3],
                                (_, false, true) => &[0, 1, 2],
                                (_, true, true) => &[0, 1, 2, 3, 4],
                            };
                        for (current_index, &current_qid) in expected_qids.iter().enumerate() {
                            let current = item(current_qid, 100 + current_qid);
                            let renderer = QConnectRendererState {
                                current_track: Some(current.clone()),
                                // A repeat-one prefetch names the same item.
                                // It must never turn a manual Next into repeat.
                                next_track: Some(current),
                                loop_mode: Some(loop_mode),
                                current_position_ms: Some(position_ms),
                                ..Default::default()
                            };
                            for (direction, target_index) in [
                                (
                                    QconnectRemoteSkipDirection::Next,
                                    next_indexes[current_index],
                                ),
                                (
                                    QconnectRemoteSkipDirection::Previous,
                                    previous_indexes[current_index],
                                ),
                            ] {
                                let result = resolve_controller_queue_item_from_snapshots(
                                    &q, &renderer, direction,
                                );
                                assert_eq!(
                                    result.target_queue_item_id,
                                    Some(expected_qids[target_index]),
                                    "shuffle={shuffle} autoplay={autoplay} loop={loop_mode} pos={position_ms} current={current_index} direction={direction:?}"
                                );
                                assert_eq!(result.queue_index, Some(target_index));
                                assert_eq!(
                                    result.matched_track_id,
                                    Some(100 + expected_qids[target_index])
                                );
                                if direction == QconnectRemoteSkipDirection::Previous {
                                    assert_eq!(
                                        result.strategy == "restart_current_queue_item",
                                        target_index == current_index
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn manual_skip_next_uses_current_cursor_instead_of_stale_prefetch_cursor() {
        let q = queue(vec![item(0, 100), item(1, 101), item(2, 102)]);
        let renderer = QConnectRendererState {
            current_track: Some(item(1, 101)),
            next_track: Some(item(0, 100)),
            ..Default::default()
        };
        assert_eq!(
            resolve_controller_queue_item_from_snapshots(
                &q,
                &renderer,
                QconnectRemoteSkipDirection::Next
            )
            .target_queue_item_id,
            Some(2)
        );
    }

    #[test]
    fn manual_skip_never_infers_current_from_prefetch_alone() {
        let q = queue(vec![item(0, 100), item(1, 101), item(2, 102)]);
        for current_track in [None, Some(item(999, 999))] {
            let renderer = QConnectRendererState {
                current_track,
                next_track: Some(item(2, 102)),
                ..Default::default()
            };
            for direction in [
                QconnectRemoteSkipDirection::Next,
                QconnectRemoteSkipDirection::Previous,
            ] {
                let result = resolve_controller_queue_item_from_snapshots(&q, &renderer, direction);
                assert_eq!(result.target_queue_item_id, None);
                assert_eq!(result.strategy, "no_current_queue_item");
            }
        }
    }

    #[test]
    fn manual_skip_one_item_queue_restarts_without_division_or_boundary_failure() {
        let q = queue(vec![item(0, 100)]);
        for loop_mode in [None, Some(1), Some(2), Some(3)] {
            for position_ms in [None, Some(4000), Some(4001)] {
                let renderer = QConnectRendererState {
                    current_track: Some(item(0, 100)),
                    loop_mode,
                    current_position_ms: position_ms,
                    ..Default::default()
                };
                for direction in [
                    QconnectRemoteSkipDirection::Next,
                    QconnectRemoteSkipDirection::Previous,
                ] {
                    let result =
                        resolve_controller_queue_item_from_snapshots(&q, &renderer, direction);
                    assert_eq!(result.target_queue_item_id, Some(0));
                    if direction == QconnectRemoteSkipDirection::Previous {
                        assert_eq!(result.strategy, "restart_current_queue_item");
                    }
                }
            }
        }
    }

    #[test]
    fn manual_skip_requires_authoritative_shuffle_order() {
        let mut q = queue(vec![item(0, 100), item(1, 101), item(2, 102)]);
        q.shuffle_mode = true;
        let renderer = QConnectRendererState {
            current_track: Some(item(1, 101)),
            next_track: Some(item(2, 102)),
            ..Default::default()
        };
        for order in [
            None,
            Some(vec![0, 0, 1]),
            Some(vec![0, 1, 3]),
            Some(vec![0, 1]),
        ] {
            q.shuffle_order = order;
            for direction in [
                QconnectRemoteSkipDirection::Next,
                QconnectRemoteSkipDirection::Previous,
            ] {
                let result = resolve_controller_queue_item_from_snapshots(&q, &renderer, direction);
                assert_eq!(result.target_queue_item_id, None);
                assert_eq!(result.strategy, "missing_authoritative_shuffle_order");
            }
        }
    }

    #[test]
    fn manual_skip_handles_empty_main_queue_without_inventing_repeat_targets() {
        let mut q = queue(Vec::new());
        let mut renderer = QConnectRendererState {
            current_track: Some(item(4, 104)),
            ..Default::default()
        };
        for direction in [
            QconnectRemoteSkipDirection::Next,
            QconnectRemoteSkipDirection::Previous,
        ] {
            assert_eq!(
                resolve_controller_queue_item_from_snapshots(&q, &renderer, direction).strategy,
                "no_queue_items"
            );
        }
        q.autoplay_items = vec![item(4, 104), item(5, 105)];
        for loop_mode in [1, 2, 3] {
            renderer.loop_mode = Some(loop_mode);
            for (direction, qid) in [
                (QconnectRemoteSkipDirection::Next, 5),
                (QconnectRemoteSkipDirection::Previous, 4),
            ] {
                let result = resolve_controller_queue_item_from_snapshots(&q, &renderer, direction);
                assert_eq!(result.target_queue_item_id, (loop_mode != 3).then_some(qid));
                if loop_mode == 3 {
                    assert_eq!(result.strategy, "no_repeat_queue_items");
                }
            }
        }
    }
}
