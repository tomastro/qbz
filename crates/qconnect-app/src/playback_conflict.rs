//! Persistable policy for resolving the bootstrap conflict between local
//! playback and an already-active Qobuz Connect renderer.

use serde::{Deserialize, Serialize};

use crate::app::LocalPlaybackConflictChoice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QconnectPlaybackConflictPolicy {
    /// Open the four-choice modal for every conflict. This is the fail-open
    /// default because it never makes an ownership decision on the user's
    /// behalf.
    #[default]
    AskEveryTime,
    ContinueOnActiveRenderer,
    ContinueOnThisDevice,
    ContinueLocalPlaybackAndReplaceQueue,
}

impl QconnectPlaybackConflictPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AskEveryTime => "ask_every_time",
            Self::ContinueOnActiveRenderer => "continue_on_active_renderer",
            Self::ContinueOnThisDevice => "continue_on_this_device",
            Self::ContinueLocalPlaybackAndReplaceQueue => {
                "continue_local_playback_and_replace_queue"
            }
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "ask_every_time" => Some(Self::AskEveryTime),
            "continue_on_active_renderer" => Some(Self::ContinueOnActiveRenderer),
            "continue_on_this_device" => Some(Self::ContinueOnThisDevice),
            "continue_local_playback_and_replace_queue" => {
                Some(Self::ContinueLocalPlaybackAndReplaceQueue)
            }
            _ => None,
        }
    }

    pub const fn index(self) -> i32 {
        match self {
            Self::AskEveryTime => 0,
            Self::ContinueOnActiveRenderer => 1,
            Self::ContinueOnThisDevice => 2,
            Self::ContinueLocalPlaybackAndReplaceQueue => 3,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::AskEveryTime),
            1 => Some(Self::ContinueOnActiveRenderer),
            2 => Some(Self::ContinueOnThisDevice),
            3 => Some(Self::ContinueLocalPlaybackAndReplaceQueue),
            _ => None,
        }
    }

    /// `None` means the frontend must ask. CancelConnection is deliberately
    /// absent: cancelling a connection is a one-shot modal action, not a
    /// sensible automatic policy.
    pub const fn automatic_choice(self) -> Option<LocalPlaybackConflictChoice> {
        match self {
            Self::AskEveryTime => None,
            Self::ContinueOnActiveRenderer => {
                Some(LocalPlaybackConflictChoice::ContinueOnActiveRenderer)
            }
            Self::ContinueOnThisDevice => Some(LocalPlaybackConflictChoice::ContinueOnThisDevice),
            Self::ContinueLocalPlaybackAndReplaceQueue => {
                Some(LocalPlaybackConflictChoice::ContinueLocalPlaybackAndReplaceQueue)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_asks_every_time() {
        assert_eq!(
            QconnectPlaybackConflictPolicy::default(),
            QconnectPlaybackConflictPolicy::AskEveryTime
        );
        assert_eq!(
            QconnectPlaybackConflictPolicy::default().automatic_choice(),
            None
        );
    }

    #[test]
    fn values_round_trip_through_storage_and_ui_index() {
        let policies = [
            QconnectPlaybackConflictPolicy::AskEveryTime,
            QconnectPlaybackConflictPolicy::ContinueOnActiveRenderer,
            QconnectPlaybackConflictPolicy::ContinueOnThisDevice,
            QconnectPlaybackConflictPolicy::ContinueLocalPlaybackAndReplaceQueue,
        ];
        for policy in policies {
            assert_eq!(
                QconnectPlaybackConflictPolicy::from_str(policy.as_str()),
                Some(policy)
            );
            assert_eq!(
                QconnectPlaybackConflictPolicy::from_index(policy.index() as usize),
                Some(policy)
            );
        }
    }

    #[test]
    fn cancel_is_not_an_automatic_policy() {
        for index in 1..=3 {
            assert_ne!(
                QconnectPlaybackConflictPolicy::from_index(index)
                    .and_then(QconnectPlaybackConflictPolicy::automatic_choice),
                Some(LocalPlaybackConflictChoice::CancelConnection)
            );
        }
        assert_eq!(QconnectPlaybackConflictPolicy::from_index(4), None);
    }
}
