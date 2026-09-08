//! Read-through cache for profile-scoped media-server metadata sidecars.
//!
//! Remote rows are mapped on paging paths where a SQLite lookup per item would
//! be a severe regression. The small sidecar database is therefore decoded
//! once per active profile and indexed by `(source, server, album)`; a save
//! replaces and reloads the snapshot atomically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use qbz_app::settings::media_servers::MediaServerKind;
use qbz_library::{AlbumTagSidecar, LocalTrack, RemoteTagSidecarStore, RemoteTagTarget};

/// A cached local file stored in a remote sidecar must retain LOCAL artwork
/// provenance. Without this marker, the artwork window hands `/cache/foo.jpg`
/// to Plex/Jellyfin as if it were a server token.
pub const LOCAL_ART_PREFIX: &str = qbz_local_catalog::SIDECAR_LOCAL_ART_PREFIX;

#[derive(Default)]
struct Snapshot {
    path: Option<PathBuf>,
    albums: HashMap<RemoteTagTarget, AlbumTagSidecar>,
}

static SNAPSHOT: LazyLock<RwLock<Snapshot>> = LazyLock::new(|| RwLock::new(Snapshot::default()));

pub fn path() -> Option<PathBuf> {
    crate::local_state::db_path().and_then(|path| {
        path.parent()
            .map(|parent| parent.join("metadata_sidecars.db"))
    })
}

fn load(path: &Path) -> HashMap<RemoteTagTarget, AlbumTagSidecar> {
    if !path.is_file() {
        return HashMap::new();
    }
    match RemoteTagSidecarStore::open(path).and_then(|store| store.all()) {
        Ok(rows) => rows
            .into_iter()
            .map(|stored| (stored.target, stored.sidecar))
            .collect(),
        Err(error) => {
            log::warn!("[qbz-qt] remote metadata sidecar load failed: {error}");
            HashMap::new()
        }
    }
}

fn ensure_profile() {
    let path = path();
    let needs_reload = SNAPSHOT
        .read()
        .map(|snapshot| snapshot.path != path)
        .unwrap_or(true);
    if !needs_reload {
        return;
    }
    let albums = path.as_deref().map(load).unwrap_or_default();
    if let Ok(mut snapshot) = SNAPSHOT.write() {
        if snapshot.path != path {
            snapshot.path = path;
            snapshot.albums = albums;
        }
    }
}

pub fn reload() {
    let path = path();
    let albums = path.as_deref().map(load).unwrap_or_default();
    if let Ok(mut snapshot) = SNAPSHOT.write() {
        snapshot.path = path;
        snapshot.albums = albums;
    }
}

pub fn active_source_instance(source: &str) -> String {
    let value = match canonical_source(source) {
        "plex" => crate::local_plex::settings().machine_id,
        "jellyfin" => crate::media_servers_qt::get(MediaServerKind::Jellyfin).server_id,
        "subsonic" => crate::media_servers_qt::get(MediaServerKind::Subsonic).server_id,
        _ => String::new(),
    };
    if value.trim().is_empty() {
        "default".to_string()
    } else {
        value
    }
}

pub fn canonical_source(source: &str) -> &str {
    match source.to_ascii_lowercase().as_str() {
        "navidrome" | "gonic" | "airsonic" | "astiga" => "subsonic",
        "plex" => "plex",
        "jellyfin" => "jellyfin",
        "subsonic" => "subsonic",
        _ => "",
    }
}

pub fn target(source: &str, source_instance: &str, album_id: &str) -> RemoteTagTarget {
    RemoteTagTarget::new(canonical_source(source), source_instance, album_id)
}

pub fn sidecar(target: &RemoteTagTarget) -> Option<AlbumTagSidecar> {
    ensure_profile();
    SNAPSHOT
        .read()
        .ok()
        .and_then(|snapshot| snapshot.albums.get(target).cloned())
}

pub fn save(target: &RemoteTagTarget, sidecar: &AlbumTagSidecar) -> Result<(), String> {
    let path = path().ok_or_else(|| "active user data directory unavailable".to_string())?;
    let mut store = RemoteTagSidecarStore::open(&path).map_err(|error| error.to_string())?;
    store
        .put(target, sidecar)
        .map_err(|error| error.to_string())?;
    reload();
    Ok(())
}

/// Apply the effective sidecar to a mapped remote row without doing I/O.
pub fn apply(track: &mut LocalTrack, source: &str, source_instance: &str, album_id: &str) {
    let target = target(source, source_instance, album_id);
    let Some(sidecar) = sidecar(&target) else {
        return;
    };
    qbz_library::apply_sidecar_to_track(track, &sidecar);
    if let Some(path) = sidecar
        .extended_album
        .as_ref()
        .and_then(|album| album.artwork_path.as_deref())
        .filter(|path| !path.trim().is_empty())
    {
        track.artwork_path = Some(format!("{LOCAL_ART_PREFIX}{path}"));
    }
}

pub fn local_art_path(token: &str) -> Option<&str> {
    token.strip_prefix(LOCAL_ART_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navidrome_family_uses_the_subsonic_sidecar_namespace() {
        for word in ["navidrome", "gonic", "airsonic", "astiga", "subsonic"] {
            assert_eq!(canonical_source(word), "subsonic");
        }
    }

    #[test]
    fn local_art_marker_is_unambiguous_and_reversible() {
        let token = format!("{LOCAL_ART_PREFIX}/cache/qbz/cover.jpg");
        assert_eq!(local_art_path(&token), Some("/cache/qbz/cover.jpg"));
        assert_eq!(local_art_path("/Items/12/Images/Primary"), None);
    }
}
