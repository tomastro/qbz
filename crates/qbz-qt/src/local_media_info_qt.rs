//! Physical-media information for Local Library tracks and logical albums.
//!
//! This is deliberately separate from the Qobuz Track/Album Info modals:
//! those describe the catalog record (credits, label, review), while this
//! document answers where the playable media comes from, how large it is and
//! which local path or server-native identity owns it.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use cxx_qt_lib::QString;
use qbz_app::settings::media_servers::MediaServerKind;
use qbz_library::LocalTrack;
use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaLocation {
    kind: &'static str,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaInfoDoc {
    kind: &'static str,
    title: String,
    subtitle: String,
    source_kinds: Vec<&'static str>,
    server: String,
    track_count: usize,
    duration: String,
    formats: String,
    quality: String,
    channels: String,
    file_size: String,
    file_size_bytes: u64,
    locations: Vec<MediaLocation>,
    error: String,
}

fn remote_source(track: &LocalTrack) -> &'static str {
    let source =
        crate::remote_metadata_qt::canonical_source(track.source.as_deref().unwrap_or_default());
    if !source.is_empty() {
        return match source {
            "plex" => "plex",
            "jellyfin" => "jellyfin",
            "subsonic" => "subsonic",
            _ => "",
        };
    }
    track
        .album_group_key
        .split_once(':')
        .map(|(prefix, _)| crate::remote_metadata_qt::canonical_source(prefix))
        .map(|source| match source {
            "plex" => "plex",
            "jellyfin" => "jellyfin",
            "subsonic" => "subsonic",
            _ => "",
        })
        .unwrap_or_default()
}

fn source_kind(track: &LocalTrack) -> &'static str {
    let remote = remote_source(track);
    if !remote.is_empty() {
        return remote;
    }
    match track.source.as_deref().unwrap_or_default() {
        "qobuz_download" | "qobuz_purchase" => "offline",
        _ if track.is_network_mount => "network",
        _ => "local",
    }
}

fn server_label(kind: &str) -> String {
    match kind {
        "plex" => crate::local_plex::settings().base_url,
        "jellyfin" => {
            let settings = crate::media_servers_qt::get(MediaServerKind::Jellyfin);
            if settings.server_name.trim().is_empty() {
                settings.base_url
            } else if settings.base_url.trim().is_empty() {
                settings.server_name
            } else {
                format!("{} · {}", settings.server_name, settings.base_url)
            }
        }
        "subsonic" => {
            let settings = crate::media_servers_qt::get(MediaServerKind::Subsonic);
            if settings.server_name.trim().is_empty() {
                settings.base_url
            } else if settings.base_url.trim().is_empty() {
                settings.server_name
            } else {
                format!("{} · {}", settings.server_name, settings.base_url)
            }
        }
        _ => String::new(),
    }
}

fn human_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return String::new();
    }
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn physical_size(track: &LocalTrack) -> u64 {
    if track.file_size_bytes > 0 {
        return track.file_size_bytes;
    }
    if !remote_source(track).is_empty() {
        return 0;
    }
    std::fs::metadata(&track.file_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn remote_album_id(track: &LocalTrack) -> String {
    let source = remote_source(track);
    if source == "plex" {
        return track
            .album_group_key
            .strip_prefix("plex:album:")
            .or_else(|| track.album_group_key.strip_prefix("plex:"))
            .unwrap_or(&track.album_group_key)
            .to_string();
    }
    track
        .album_group_key
        .strip_prefix(&format!("{source}:"))
        .unwrap_or(&track.album_group_key)
        .to_string()
}

fn locations_for_track(track: &LocalTrack) -> Vec<MediaLocation> {
    if !remote_source(track).is_empty() {
        return vec![
            MediaLocation {
                kind: "item",
                value: track.file_path.clone(),
            },
            MediaLocation {
                kind: "album",
                value: remote_album_id(track),
            },
        ];
    }
    let mut locations = vec![MediaLocation {
        kind: "file",
        value: track.file_path.clone(),
    }];
    if let Some(parent) = Path::new(&track.file_path).parent() {
        locations.push(MediaLocation {
            kind: "folder",
            value: parent.to_string_lossy().into_owned(),
        });
    }
    locations
}

fn aggregate_doc(kind: &'static str, tracks: &[LocalTrack]) -> MediaInfoDoc {
    let Some(first) = tracks.first() else {
        return MediaInfoDoc {
            kind,
            title: String::new(),
            subtitle: String::new(),
            source_kinds: Vec::new(),
            server: String::new(),
            track_count: 0,
            duration: String::new(),
            formats: String::new(),
            quality: String::new(),
            channels: String::new(),
            file_size: String::new(),
            file_size_bytes: 0,
            locations: Vec::new(),
            error: qbz_i18n::t("The media item is no longer available."),
        };
    };

    let mut source_kinds = BTreeSet::new();
    let mut formats = BTreeSet::new();
    let mut quality = BTreeSet::new();
    let mut channels = BTreeSet::new();
    let mut servers = BTreeSet::new();
    let mut seen_files = HashSet::new();
    let mut file_size_bytes = 0u64;
    let mut duration_secs = 0u64;
    let mut locations = Vec::new();
    let mut seen_locations = HashSet::new();

    for track in tracks {
        let source = source_kind(track);
        source_kinds.insert(source);
        let server = server_label(source);
        if !server.trim().is_empty() {
            servers.insert(server);
        }
        formats.insert(track.format.to_string());
        let detail =
            crate::local_rows::detail_of(&track.format, track.bit_depth, track.sample_rate);
        if !detail.trim().is_empty() {
            quality.insert(detail);
        }
        if track.channels > 0 {
            channels.insert(track.channels.to_string());
        }
        duration_secs = duration_secs.saturating_add(track.duration_secs);

        // CUE rows can share one physical file. Count that file once while
        // retaining every logical track in the duration/track count.
        let size_key = if remote_source(track).is_empty() {
            track.file_path.clone()
        } else {
            format!("{}:{}", source, track.file_path)
        };
        if seen_files.insert(size_key) {
            file_size_bytes = file_size_bytes.saturating_add(physical_size(track));
        }

        let candidates = if kind == "track" {
            locations_for_track(track)
        } else if !remote_source(track).is_empty() {
            vec![MediaLocation {
                kind: "album",
                value: remote_album_id(track),
            }]
        } else {
            Path::new(&track.file_path)
                .parent()
                .map(|directory| {
                    vec![MediaLocation {
                        kind: "folder",
                        value: directory.to_string_lossy().into_owned(),
                    }]
                })
                .unwrap_or_default()
        };
        for location in candidates {
            if seen_locations.insert((location.kind, location.value.clone())) {
                locations.push(location);
            }
        }
    }

    let title = if kind == "track" {
        first.title.clone()
    } else if first.album_group_title.trim().is_empty() {
        first.album.clone()
    } else {
        first.album_group_title.clone()
    };
    let subtitle = if kind == "track" {
        [first.artist.as_str(), first.album_group_title.as_str()]
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" — ")
    } else {
        first
            .album_artist
            .as_deref()
            .filter(|artist| !artist.trim().is_empty())
            .unwrap_or(&first.artist)
            .to_string()
    };

    MediaInfoDoc {
        kind,
        title,
        subtitle,
        source_kinds: source_kinds.into_iter().collect(),
        server: servers.into_iter().collect::<Vec<_>>().join("\n"),
        track_count: tracks.len(),
        duration: if kind == "track" {
            crate::local_rows::mmss(duration_secs)
        } else {
            crate::local_rows::total_duration(duration_secs)
        },
        formats: formats.into_iter().collect::<Vec<_>>().join(", "),
        quality: quality.into_iter().collect::<Vec<_>>().join(", "),
        channels: channels.into_iter().collect::<Vec<_>>().join(", "),
        file_size: human_bytes(file_size_bytes),
        file_size_bytes,
        locations,
        error: String::new(),
    }
}

fn publish(doc: MediaInfoDoc) {
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string());
    crate::local_bridge::ui(move |mut bridge| {
        bridge
            .as_mut()
            .set_local_media_info_json(QString::from(json.as_str()));
        bridge.as_mut().set_local_media_info_loading(false);
        bridge.as_mut().set_local_media_info_open(true);
    });
}

pub fn begin() {
    crate::local_bridge::ui(|mut bridge| {
        bridge
            .as_mut()
            .set_local_media_info_json(QString::from("{}"));
        bridge.as_mut().set_local_media_info_loading(true);
        bridge.as_mut().set_local_media_info_open(true);
    });
}

pub fn open_track(track: LocalTrack) {
    publish(aggregate_doc("track", &[track]));
}

pub fn open_album(tracks: Vec<LocalTrack>) {
    publish(aggregate_doc("album", &tracks));
}

pub fn open_empty(kind: &'static str) {
    publish(aggregate_doc(kind, &[]));
}

pub fn close() {
    crate::local_bridge::ui(|mut bridge| {
        bridge.as_mut().set_local_media_info_open(false);
        bridge.as_mut().set_local_media_info_loading(false);
    });
}

pub fn copy(value: String) {
    if value.trim().is_empty() {
        return;
    }
    crate::share_qt::copy_to_clipboard(value);
    crate::toast_qt::success(qbz_i18n::t("Copied!"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_namespace_never_becomes_a_filesystem_location() {
        let track = LocalTrack {
            title: "Remote".into(),
            file_path: "item-7".into(),
            album_group_key: "jellyfin:album-3".into(),
            source: None,
            ..LocalTrack::default()
        };
        let doc = aggregate_doc("track", &[track]);
        assert_eq!(doc.source_kinds, ["jellyfin"]);
        assert_eq!(doc.locations[0].kind, "item");
        assert_eq!(doc.locations[0].value, "item-7");
        assert_eq!(doc.locations[1].value, "album-3");
    }

    #[test]
    fn cue_rows_count_one_physical_file_size() {
        let first = LocalTrack {
            id: 1,
            file_path: "/music/image.flac".into(),
            album_group_key: "/music".into(),
            file_size_bytes: 1_048_576,
            cue_start_secs: Some(0.0),
            ..LocalTrack::default()
        };
        let mut second = first.clone();
        second.id = 2;
        second.cue_start_secs = Some(180.0);
        let doc = aggregate_doc("album", &[first, second]);
        assert_eq!(doc.track_count, 2);
        assert_eq!(doc.file_size_bytes, 1_048_576);
        assert_eq!(doc.file_size, "1.00 MiB");
    }
}
