//! Playback context model.
//!
//! A playback context describes the semantic origin of playback. It is not the
//! queue itself; it is the source boundary used by commands that need to know
//! whether the current playback came from an album, playlist, radio session,
//! search result, or another app-level source.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextType {
    Album,
    Playlist,
    ArtistTop,
    LabelTop,
    HomeList,
    DailyQ,
    WeeklyQ,
    FavQ,
    TopQ,
    Favorites,
    LocalLibrary,
    Radio,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentSource {
    Qobuz,
    Local,
    Plex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackContext {
    #[serde(rename = "type")]
    pub context_type: ContextType,
    pub id: String,
    pub label: String,
    pub source: ContentSource,
    pub track_ids: Vec<u64>,
    pub current_position: usize,
}

impl PlaybackContext {
    pub fn new(
        context_type: ContextType,
        id: String,
        label: String,
        source: ContentSource,
        track_ids: Vec<u64>,
        start_position: usize,
    ) -> Self {
        Self {
            context_type,
            id,
            label,
            source,
            track_ids,
            current_position: start_position,
        }
    }

    pub fn next_track_id(&self) -> Option<u64> {
        let next_pos = self.current_position + 1;
        if next_pos < self.track_ids.len() {
            self.track_ids.get(next_pos).copied()
        } else {
            None
        }
    }

    pub fn upcoming_track_ids(&self, count: usize) -> Vec<u64> {
        let start_pos = self.current_position + 1;
        self.track_ids
            .iter()
            .skip(start_pos)
            .take(count)
            .copied()
            .collect()
    }

    pub fn advance(&mut self) -> bool {
        let next_pos = self.current_position + 1;
        if next_pos < self.track_ids.len() {
            self.current_position = next_pos;
            true
        } else {
            false
        }
    }

    pub fn has_next(&self) -> bool {
        self.current_position + 1 < self.track_ids.len()
    }

    pub fn total_tracks(&self) -> usize {
        self.track_ids.len()
    }

    pub fn display_info(&self) -> String {
        let type_str = match self.context_type {
            ContextType::Album => "Album",
            ContextType::Playlist => "Playlist",
            ContextType::ArtistTop => "Artist Top Songs",
            ContextType::LabelTop => "Label Top Songs",
            ContextType::HomeList => "Home List",
            ContextType::DailyQ => "DailyQ",
            ContextType::WeeklyQ => "WeeklyQ",
            ContextType::FavQ => "FavQ",
            ContextType::TopQ => "TopQ",
            ContextType::Favorites => "Favorites",
            ContextType::LocalLibrary => "Local Library",
            ContextType::Radio => "Radio",
            ContextType::Search => "Search Results",
        };
        format!("{} · {}", type_str, self.label)
    }
}

pub struct ContextManager {
    current: Mutex<Option<PlaybackContext>>,
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }

    pub fn set_context(&self, context: PlaybackContext) {
        let mut current = self.current.lock().unwrap();
        *current = Some(context);
        log::info!(
            "Playback context set: {:?}",
            current.as_ref().map(|c| &c.label)
        );
    }

    pub fn clear_context(&self) {
        let mut current = self.current.lock().unwrap();
        *current = None;
        log::info!("Playback context cleared");
    }

    pub fn get_context(&self) -> Option<PlaybackContext> {
        self.current.lock().unwrap().clone()
    }

    pub fn has_context(&self) -> bool {
        self.current.lock().unwrap().is_some()
    }

    pub fn next_track_id(&self) -> Option<u64> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|ctx| ctx.next_track_id())
    }

    pub fn upcoming_track_ids(&self, count: usize) -> Vec<u64> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .map(|ctx| ctx.upcoming_track_ids(count))
            .unwrap_or_default()
    }

    pub fn advance_context(&self) -> bool {
        let mut current = self.current.lock().unwrap();
        if let Some(ctx) = current.as_mut() {
            let advanced = ctx.advance();
            if !advanced {
                log::info!("Playback context ended (no more tracks)");
            }
            advanced
        } else {
            false
        }
    }

    pub fn set_position(&self, track_id: u64) {
        let mut current = self.current.lock().unwrap();
        if let Some(ctx) = current.as_mut() {
            if let Some(pos) = ctx.track_ids.iter().position(|&id| id == track_id) {
                ctx.current_position = pos;
                log::debug!("Context position updated to {}", pos);
            }
        }
    }

    pub fn append_track_ids(&self, new_track_ids: Vec<u64>) {
        let mut current = self.current.lock().unwrap();
        if let Some(ctx) = current.as_mut() {
            let count = new_track_ids.len();
            ctx.track_ids.extend(new_track_ids);
            log::debug!(
                "Appended {} track IDs to context (total: {})",
                count,
                ctx.track_ids.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album_context() -> PlaybackContext {
        PlaybackContext::new(
            ContextType::Album,
            "album-1".to_string(),
            "Album Title".to_string(),
            ContentSource::Qobuz,
            vec![10, 11, 12, 13],
            1,
        )
    }

    #[test]
    fn playback_context_reports_next_and_upcoming_tracks() {
        let context = album_context();

        assert_eq!(context.next_track_id(), Some(12));
        assert_eq!(context.upcoming_track_ids(2), vec![12, 13]);
        assert_eq!(context.upcoming_track_ids(10), vec![12, 13]);
        assert!(context.has_next());
        assert_eq!(context.total_tracks(), 4);
    }

    #[test]
    fn playback_context_advance_updates_position_until_end() {
        let mut context = album_context();

        assert!(context.advance());
        assert_eq!(context.current_position, 2);
        assert_eq!(context.next_track_id(), Some(13));
        assert!(context.advance());
        assert_eq!(context.current_position, 3);
        assert_eq!(context.next_track_id(), None);
        assert!(!context.has_next());
        assert!(!context.advance());
        assert_eq!(context.current_position, 3);
    }

    #[test]
    fn playback_context_display_info_matches_existing_labels() {
        assert_eq!(album_context().display_info(), "Album · Album Title");

        let radio = PlaybackContext::new(
            ContextType::Radio,
            "radio-1".to_string(),
            "Seed".to_string(),
            ContentSource::Qobuz,
            vec![1],
            0,
        );
        assert_eq!(radio.display_info(), "Radio · Seed");
    }

    #[test]
    fn context_manager_sets_clears_and_reports_context() {
        let manager = ContextManager::new();
        assert!(!manager.has_context());
        assert_eq!(manager.next_track_id(), None);
        assert!(manager.upcoming_track_ids(3).is_empty());
        assert!(!manager.advance_context());

        manager.set_context(album_context());

        assert!(manager.has_context());
        assert_eq!(manager.next_track_id(), Some(12));
        assert_eq!(manager.upcoming_track_ids(3), vec![12, 13]);
        assert_eq!(
            manager.get_context().map(|ctx| ctx.label),
            Some("Album Title".to_string())
        );

        manager.clear_context();
        assert!(!manager.has_context());
    }

    #[test]
    fn context_manager_updates_position_by_track_id() {
        let manager = ContextManager::new();
        manager.set_context(album_context());

        manager.set_position(13);
        let context = manager.get_context().expect("context exists");
        assert_eq!(context.current_position, 3);
        assert_eq!(context.next_track_id(), None);

        manager.set_position(999);
        let context = manager.get_context().expect("context exists");
        assert_eq!(context.current_position, 3);
    }

    #[test]
    fn context_manager_appends_radio_refill_track_ids() {
        let manager = ContextManager::new();
        manager.set_context(album_context());

        manager.append_track_ids(vec![14, 15]);
        let context = manager.get_context().expect("context exists");

        assert_eq!(context.track_ids, vec![10, 11, 12, 13, 14, 15]);
        assert_eq!(context.upcoming_track_ids(10), vec![12, 13, 14, 15]);
    }
}
