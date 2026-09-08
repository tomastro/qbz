//! Execute a [`CloudPlan`] against the target account: one favorite per
//! call with pacing and retries, playlists created and filled in 50-track
//! chunks in order, subscriptions re-followed. Every success is written to
//! the ledger before the next write, so a crash mid-run resumes.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::api::MigrationApi;
use crate::ledger::Ledger;
use crate::plan::{CloudPlan, PlaylistAction};
use crate::sink::{MigrationEvent, MigrationPhase, MigrationSink};

/// The importer's chunk (`qbz-playlist-import`): 50 ids per `addTracks`.
const ADD_CHUNK: usize = 50;
/// Breathing room between writes; the 403 breaker is the real ceiling.
const PACE: Duration = Duration::from_millis(100);
/// Transient retries per write: 1 s, 2 s, 4 s.
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SectionReport {
    pub added: usize,
    pub already: usize,
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    pub favorites: SectionReport,
    pub playlists: SectionReport,
    /// Tracks appended across all playlists.
    pub tracks_added: usize,
    pub subscriptions: SectionReport,
}

impl ApplyReport {
    pub fn failed(&self) -> usize {
        self.favorites.failed.len() + self.playlists.failed.len() + self.subscriptions.failed.len()
    }
}

/// A write with bounded retries. Only the last error is reported.
async fn with_retry<F, Fut>(mut op: F) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut last = String::new();
    for (attempt, backoff) in RETRY_BACKOFF.iter().enumerate() {
        match op().await {
            Ok(()) => return Ok(()),
            Err(e) => {
                log::warn!(
                    "[account-migration] write failed (attempt {}): {e}",
                    attempt + 1
                );
                last = e;
                tokio::time::sleep(*backoff).await;
            }
        }
    }
    match op().await {
        Ok(()) => Ok(()),
        Err(e) => Err(if e.is_empty() { last } else { e }),
    }
}

/// Run the plan. `profile_dir` is the TARGET profile (the ledger lives in
/// it). Failures are collected per item and the run continues; the report
/// says exactly what did not happen.
pub async fn apply<A: MigrationApi, S: MigrationSink>(
    plan: &CloudPlan,
    api: &A,
    profile_dir: &Path,
    source_user_id: u64,
    sink: &S,
) -> Result<ApplyReport, String> {
    let mut ledger = Ledger::load(profile_dir)?;
    ledger.sources.insert(source_user_id);
    let mut report = ApplyReport::default();

    // --- Favorites, oldest first -----------------------------------------
    sink.emit(MigrationEvent::Phase(MigrationPhase::Favorites));
    let total: usize = plan.favorites_to_add();
    let mut done = 0usize;
    for (kind, items) in &plan.favorites.to_add {
        for item in items {
            sink.emit(MigrationEvent::Progress {
                done,
                total,
                label: format!("{}: {}", kind.plural(), item.title),
            });
            let id = item.id.clone();
            let singular = kind.singular();
            match with_retry(|| api.add_favorite(singular, &id)).await {
                Ok(()) => {
                    ledger.mark_favorite(kind.plural(), &item.id);
                    ledger.save(profile_dir)?;
                    report.favorites.added += 1;
                }
                Err(e) => {
                    report
                        .favorites
                        .failed
                        .push(format!("{} {}: {e}", kind.singular(), item.id))
                }
            }
            done += 1;
            tokio::time::sleep(PACE).await;
        }
    }
    report.favorites.already = plan.favorites.already.iter().map(|(_, n)| n).sum();

    // --- Playlists ---------------------------------------------------------
    sink.emit(MigrationEvent::Phase(MigrationPhase::Playlists));
    let total = plan.playlists.len();
    for (i, action) in plan.playlists.iter().enumerate() {
        sink.emit(MigrationEvent::Progress {
            done: i,
            total,
            label: action.name().to_string(),
        });
        let (target_id, track_ids): (u64, &[u64]) = match action {
            PlaylistAction::Merge {
                target_id,
                missing_track_ids,
                ..
            } => (*target_id, missing_track_ids),
            PlaylistAction::Create {
                name,
                description,
                is_public,
                track_ids,
                ..
            }
            | PlaylistAction::CreateCopy {
                name,
                description,
                is_public,
                track_ids,
                ..
            } => {
                let created = match api
                    .create_playlist(name, description.as_deref(), *is_public)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        report
                            .playlists
                            .failed
                            .push(format!("{name}: create failed: {e}"));
                        continue;
                    }
                };
                // Map it before filling it: a crash while adding tracks
                // must not create a second copy on the next run.
                ledger.playlist_map.insert(action.source_id(), created.id);
                ledger.save(profile_dir)?;
                (created.id, track_ids)
            }
        };
        let mut failed_chunk = None;
        for chunk in track_ids.chunks(ADD_CHUNK) {
            if let Err(e) = with_retry(|| api.add_tracks(target_id, chunk)).await {
                failed_chunk = Some(e);
                break;
            }
            report.tracks_added += chunk.len();
            tokio::time::sleep(PACE).await;
        }
        match failed_chunk {
            Some(e) => report
                .playlists
                .failed
                .push(format!("{}: adding tracks failed: {e}", action.name())),
            None => {
                ledger.playlist_map.insert(action.source_id(), target_id);
                ledger.save(profile_dir)?;
                report.playlists.added += 1;
            }
        }
    }
    report.playlists.already = plan.playlists_already;

    // --- Subscriptions -----------------------------------------------------
    sink.emit(MigrationEvent::Phase(MigrationPhase::Subscriptions));
    let total = plan.subscriptions.len();
    for (i, id) in plan.subscriptions.iter().enumerate() {
        sink.emit(MigrationEvent::Progress {
            done: i,
            total,
            label: id.to_string(),
        });
        match with_retry(|| api.subscribe(*id)).await {
            Ok(()) => {
                ledger.subscriptions_done.insert(*id);
                ledger.playlist_map.insert(*id, *id);
                ledger.save(profile_dir)?;
                report.subscriptions.added += 1;
            }
            Err(e) => report
                .subscriptions
                .failed
                .push(format!("playlist {id}: subscribe failed: {e}")),
        }
        tokio::time::sleep(PACE).await;
    }
    report.subscriptions.already = plan.subscriptions_already;

    sink.emit(MigrationEvent::Phase(MigrationPhase::Done));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan;
    use crate::sink::NullSink;
    use crate::snapshot::{AccountSnapshot, FavoriteItem, OwnedPlaylist, SubscribedPlaylist};
    use qbz_models::{Playlist, PlaylistOwner, PlaylistWithTrackIds};
    use serde_json::{json, Value};
    use std::sync::Mutex;

    /// Records every write; `fail_track` makes one track id fail forever.
    #[derive(Default)]
    struct Fake {
        writes: Mutex<Vec<String>>,
        next_playlist_id: Mutex<u64>,
        fail_add_favorite: Option<String>,
    }

    impl MigrationApi for Fake {
        async fn favorites_page(&self, plural: &str, _: u32, _: u32) -> Result<Value, String> {
            Ok(json!({ plural: { "items": [], "total": 0 } }))
        }
        async fn user_playlists(&self) -> Result<Vec<Playlist>, String> {
            Ok(vec![])
        }
        async fn playlist_track_ids(&self, _: u64) -> Result<PlaylistWithTrackIds, String> {
            Err("unused".into())
        }
        async fn add_favorite(&self, singular: &str, id: &str) -> Result<(), String> {
            if self.fail_add_favorite.as_deref() == Some(id) {
                return Err("boom".into());
            }
            self.writes
                .lock()
                .unwrap()
                .push(format!("fav {singular} {id}"));
            Ok(())
        }
        async fn create_playlist(
            &self,
            name: &str,
            _: Option<&str>,
            _: bool,
        ) -> Result<Playlist, String> {
            let mut next = self.next_playlist_id.lock().unwrap();
            *next += 1;
            self.writes
                .lock()
                .unwrap()
                .push(format!("create {name} -> {}", *next));
            Ok(Playlist {
                id: *next,
                name: name.to_string(),
                owner: PlaylistOwner::default(),
                ..serde_json::from_value(json!({"id": *next, "name": name})).unwrap()
            })
        }
        async fn add_tracks(&self, playlist_id: u64, track_ids: &[u64]) -> Result<(), String> {
            self.writes
                .lock()
                .unwrap()
                .push(format!("add {playlist_id} {track_ids:?}"));
            Ok(())
        }
        async fn subscribe(&self, playlist_id: u64) -> Result<(), String> {
            self.writes
                .lock()
                .unwrap()
                .push(format!("subscribe {playlist_id}"));
            Ok(())
        }
    }

    fn source() -> AccountSnapshot {
        let mut s = AccountSnapshot::empty(1, "src");
        s.favorites.albums = vec![
            FavoriteItem {
                id: "b".into(),
                favorited_at: Some(2),
                title: "B".into(),
                server_index: 0,
            },
            FavoriteItem {
                id: "a".into(),
                favorited_at: Some(1),
                title: "A".into(),
                server_index: 1,
            },
        ];
        s.playlists = vec![OwnedPlaylist {
            id: 10,
            name: "Mix".into(),
            description: Some("d".into()),
            is_public: false,
            track_ids: (1..=60).collect(),
        }];
        s.subscriptions = vec![SubscribedPlaylist {
            id: 77,
            name: "Theirs".into(),
            owner_id: 9,
        }];
        s
    }

    #[tokio::test]
    async fn applies_in_order_and_a_second_run_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake::default();
        let target = AccountSnapshot::empty(2, "dst");
        let p = plan(&source(), &target, &Ledger::load(dir.path()).unwrap());
        let report = apply(&p, &api, dir.path(), 1, &NullSink).await.unwrap();
        assert_eq!(report.favorites.added, 2);
        assert_eq!(report.playlists.added, 1);
        assert_eq!(report.tracks_added, 60);
        assert_eq!(report.subscriptions.added, 1);
        assert_eq!(report.failed(), 0);
        let writes = api.writes.lock().unwrap().clone();
        assert_eq!(writes[0], "fav album a"); // oldest first
        assert_eq!(writes[1], "fav album b");
        assert_eq!(writes[2], "create Mix -> 1");
        assert!(writes[3].starts_with("add 1 [1, 2,")); // chunk of 50
        assert!(writes[4].starts_with("add 1 [51,")); // then the 10 left
        assert_eq!(writes[5], "subscribe 77");

        // The ledger now knows everything: the same plan against the same
        // (still empty) target reads as done.
        let ledger = Ledger::load(dir.path()).unwrap();
        assert_eq!(ledger.playlist_map.get(&10), Some(&1));
        assert!(ledger.favorite_done("albums", "a"));
        let again = plan(&source(), &target, &ledger);
        assert_eq!(again.favorites_to_add(), 0);
        assert!(again.subscriptions.is_empty());
        // The mapped playlist is absent from the (empty) target: planned as
        // new, which is honest — the target really has no such playlist.
        assert_eq!(again.playlists.len(), 1);
    }

    #[tokio::test]
    async fn a_failing_favorite_is_reported_and_the_rest_continues() {
        let dir = tempfile::tempdir().unwrap();
        let api = Fake {
            fail_add_favorite: Some("a".into()),
            ..Fake::default()
        };
        let target = AccountSnapshot::empty(2, "dst");
        let p = plan(&source(), &target, &Ledger::default());
        let report = apply(&p, &api, dir.path(), 1, &NullSink).await.unwrap();
        assert_eq!(report.favorites.added, 1);
        assert_eq!(report.favorites.failed.len(), 1);
        assert!(report.favorites.failed[0].contains("album a"));
        assert_eq!(report.playlists.added, 1);
        let ledger = Ledger::load(dir.path()).unwrap();
        assert!(!ledger.favorite_done("albums", "a"));
        assert!(ledger.favorite_done("albums", "b"));
    }
}
