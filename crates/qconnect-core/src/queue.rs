use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QueueVersion {
    pub major: u64,
    pub minor: u64,
}

impl QueueVersion {
    pub const fn new(major: u64, minor: u64) -> Self {
        Self { major, minor }
    }

    pub const fn next_minor(self) -> Self {
        Self {
            major: self.major,
            minor: self.minor.saturating_add(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub track_context_uuid: String,
    pub track_id: u64,
    pub queue_item_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QConnectQueueState {
    pub version: QueueVersion,
    pub queue_items: Vec<QueueItem>,
    pub shuffle_mode: bool,
    pub shuffle_order: Option<Vec<usize>>,
    pub autoplay_mode: bool,
    pub autoplay_loading: bool,
    pub autoplay_items: Vec<QueueItem>,
    pub updated_at_ms: u64,
    /// Last `queue_hash` (field #100) reported by the server on a queue-state
    /// confirmation. Surfaced for divergence detection; see the algorithm-agnostic
    /// `queue_hashes_diverge` seam in qconnect-app (algorithm is a BLOCKING unknown).
    #[serde(default)]
    pub last_server_queue_hash: Option<Vec<u8>>,
}

impl Default for QConnectQueueState {
    fn default() -> Self {
        Self {
            version: QueueVersion::default(),
            queue_items: Vec::new(),
            shuffle_mode: false,
            shuffle_order: None,
            autoplay_mode: false,
            autoplay_loading: false,
            autoplay_items: Vec::new(),
            updated_at_ms: 0,
            last_server_queue_hash: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueEvent {
    QueueStateReplaced {
        action_uuid: Option<String>,
        state: QConnectQueueState,
    },
    TracksAdded {
        action_uuid: Option<String>,
        version: QueueVersion,
        tracks: Vec<QueueItem>,
        shuffle_seed: Option<u64>,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    TracksLoaded {
        action_uuid: Option<String>,
        version: QueueVersion,
        tracks: Vec<QueueItem>,
        queue_position: Option<u64>,
        shuffle_mode: Option<bool>,
        shuffle_seed: Option<u64>,
        shuffle_pivot_queue_item_id: Option<u64>,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    TracksInserted {
        action_uuid: Option<String>,
        version: QueueVersion,
        tracks: Vec<QueueItem>,
        insert_after: Option<u64>,
        shuffle_seed: Option<u64>,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    TracksRemoved {
        action_uuid: Option<String>,
        version: QueueVersion,
        queue_item_ids: Vec<u64>,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    TracksReordered {
        action_uuid: Option<String>,
        version: QueueVersion,
        queue_item_ids: Vec<u64>,
        insert_after: Option<u64>,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    QueueCleared {
        action_uuid: Option<String>,
        version: QueueVersion,
    },
    ShuffleModeSet {
        action_uuid: Option<String>,
        version: QueueVersion,
        shuffle_mode: bool,
        shuffle_seed: Option<u64>,
        shuffle_pivot_queue_item_id: Option<u64>,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    AutoplayModeSet {
        action_uuid: Option<String>,
        version: QueueVersion,
        autoplay_mode: bool,
        autoplay_reset: bool,
        autoplay_loading: bool,
    },
    AutoplayTracksLoaded {
        action_uuid: Option<String>,
        version: QueueVersion,
        tracks: Vec<QueueItem>,
    },
    AutoplayTracksRemoved {
        action_uuid: Option<String>,
        version: QueueVersion,
        queue_item_ids: Vec<u64>,
    },
    QueueError {
        action_uuid: Option<String>,
        version: Option<QueueVersion>,
        code: String,
        message: String,
    },
}
