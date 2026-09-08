//! Best-effort JSON.
//!
//! "Somebody exported their library as JSON and wants their playlist." There is
//! no schema to target, so this locates the track list BY SHAPE and harvests
//! fields by synonym, under hard caps that make a pathological file impossible
//! to weaponise.
//!
//! # Why shape and not depth
//!
//! The obvious rule — "the first array within two levels of the root" — is
//! wrong on real exports, and it is wrong in the direction that loses data.
//! The track array's depth is simply not stable: a flat `[...]` is 0,
//! `{tracks:[...]}` is 1, `{playlist:{tracks:[...]}}` is 2 and
//! `{data:{playlist:{items:[...]}}}` — an ordinary GraphQL envelope — is 3. A
//! hard two-level cap silently drops the last one. "First array" also has no
//! answer for a sibling `artists:[...]` or `images:[...]` that happens to come
//! earlier in document order.
//!
//! So depth is demoted from LOCATOR to DoS BOUND, and the locator becomes "the
//! largest array of track-shaped objects anywhere inside the guard".
//!
//! # The one false positive worth naming
//!
//! An array of ALBUM objects usually carries both a title and an artist, which
//! is exactly the both-fields test that admits a track. A discography dump
//! would import album names as track titles. Two things stop it:
//!
//! 1. **Container demotion.** An element that itself contains a nested
//!    track-shaped array is a CONTAINER (an album with its tracks, a section
//!    with its items), and an array of containers is disqualified. The real
//!    track list, nested one level down, wins instead.
//! 2. **The preview count.** The user sees "Found N tracks" before Import. On a
//!    discography that number is the album count, which is visibly wrong.
//!
//! With both, the shape locator is a superset of the depth rule on well-formed
//! exports. It is not unconditionally more precise, and this comment is the
//! place that says so.

use serde_json::Value;

use super::guard_size;
use crate::errors::PlaylistImportError;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

/// Depth guard for the structural walk. A BOUND, not a locator — nothing about
/// correctness depends on it, only the walk's cost.
const MAX_DEPTH: usize = 8;
/// Node-visit budget. Bounds adversarial WIDTH the way the byte wall bounds
/// size: a 16 MiB file of two-byte array elements is still millions of nodes.
/// On exhaustion the walk stops descending and keeps the best candidate found
/// so far — graceful degradation, never an error.
const MAX_NODES: usize = 2_000_000;
/// Output cap. The pipeline already splits at 2000 per playlist, so this is at
/// most five parts.
const MAX_TRACKS: usize = 10_000;
/// How far inside one element a field may hide. TWO object-descents, which is
/// exactly `item.track.album.name` — the deepest real field in any streaming
/// export. This one IS a locator and it is tight on purpose.
const ELEM_FIELD_DEPTH: usize = 2;

const TITLE_KEYS: &[&str] = &["title", "name", "track", "song", "trackname", "songname"];
const ARTIST_KEYS: &[&str] = &[
    "artist",
    "artists",
    "creator",
    "performer",
    "performers",
    "albumartist",
    "artistname",
    "by",
];
const ALBUM_KEYS: &[&str] = &["album", "release", "albumname", "albumtitle", "collection"];
const DURATION_KEYS: &[&str] = &[
    "duration",
    "durationms",
    "length",
    "time",
    "runtime",
    "durationseconds",
];
/// Object-valued keys that WRAP the real track. `track` and `song` are also
/// title synonyms — the collision is resolved by VALUE TYPE, never by name: a
/// scalar `track:"Song"` is a title, an object `track:{...}` is a wrapper.
const WRAPPER_KEYS: &[&str] = &["track", "item", "node", "song", "metadata"];
const PLAYLIST_NAME_KEYS: &[&str] = &["name", "title", "playlistname"];

/// Lowercase and drop `_`/`-`/spaces, so `duration_ms`, `durationMs` and
/// `Duration MS` are one key.
fn norm_key(k: &str) -> String {
    k.chars()
        .filter(|c| *c != '_' && *c != '-' && *c != ' ')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn parse(bytes: &[u8], filename: &str) -> Result<ImportPlaylist, PlaylistImportError> {
    guard_size(bytes)?;
    // serde_json's own 128-deep recursion limit pre-empts a stack attack during
    // the parse itself, before our walk guard ever runs.
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|e| PlaylistImportError::Parse(format!("JSON: {e}")))?;

    let mut budget = MAX_NODES;
    let mut best: Option<Candidate> = None;
    locate(&root, 0, &mut budget, &mut best, None);

    let Some(best) = best else {
        return Err(PlaylistImportError::JsonShapeUnrecognized);
    };

    let arr = best
        .array
        .as_array()
        .expect("candidate is always an array");
    let mut tracks: Vec<ImportTrack> = Vec::with_capacity(arr.len().min(MAX_TRACKS));
    for elem in arr.iter() {
        if tracks.len() >= MAX_TRACKS {
            log::warn!(
                "[qbz-playlist-import] JSON: truncated at {MAX_TRACKS} tracks ({} in the file)",
                arr.len()
            );
            break;
        }
        if let Some(t) = extract_track(elem) {
            tracks.push(t);
        }
    }

    if tracks.is_empty() {
        return Err(PlaylistImportError::EmptyPlaylist);
    }

    // The playlist name comes from the ROOT object or the array's PARENT —
    // never from inside the array, so a track title can never become the
    // playlist name.
    let name = best
        .parent_name
        .or_else(|| scalar_name(&root))
        .unwrap_or_else(|| super::file::decode::file_stem(filename));

    Ok(ImportPlaylist {
        provider: ImportProvider::Json,
        provider_id: filename.to_string(),
        name,
        description: None,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// (a) Structural pass — locate by shape
// ---------------------------------------------------------------------------

struct Candidate<'a> {
    array: &'a Value,
    shaped: usize,
    depth: usize,
    parent_name: Option<String>,
}

/// Depth-first walk. At every array, score it; keep the best.
///
/// "Best" is the greatest count of track-shaped elements; ties break to the
/// SHALLOWER array, then to document order (the first one seen wins, because a
/// later equal candidate never replaces it below).
fn locate<'a>(
    node: &'a Value,
    depth: usize,
    budget: &mut usize,
    best: &mut Option<Candidate<'a>>,
    parent_name: Option<&str>,
) {
    if depth > MAX_DEPTH || *budget == 0 {
        return;
    }
    *budget -= 1;

    match node {
        Value::Array(items) => {
            let shaped = items.iter().filter(|e| is_track_shaped(e)).count();
            // A MAJORITY must be track-shaped: one stray object inside an
            // `images` array cannot promote it.
            let qualifies = shaped >= 1 && shaped * 2 >= items.len();
            // Container demotion — see the module header. An array whose
            // elements each hold their own track-shaped array is a list of
            // albums/sections, not of tracks.
            let container_like = items.iter().take(32).any(has_nested_track_array);
            if qualifies && !container_like {
                let better = match best {
                    None => true,
                    Some(b) => shaped > b.shaped || (shaped == b.shaped && depth < b.depth),
                };
                if better {
                    *best = Some(Candidate {
                        array: node,
                        shaped,
                        depth,
                        parent_name: parent_name.map(str::to_string),
                    });
                }
            }
            // Descend regardless: the real list may be INSIDE a container
            // array, and that is precisely the case demotion exists to reach.
            for item in items {
                locate(item, depth + 1, budget, best, parent_name);
                if *budget == 0 {
                    return;
                }
            }
        }
        Value::Object(map) => {
            // This object's own name/title labels any array it holds.
            let here = scalar_name(node);
            for (_k, v) in map.iter() {
                locate(
                    v,
                    depth + 1,
                    budget,
                    best,
                    here.as_deref().or(parent_name),
                );
                if *budget == 0 {
                    return;
                }
            }
        }
        _ => {}
    }
}

/// Does this element hold a nested array that is itself track-shaped?
/// Checked ONE level down only — that is where an album keeps its `tracks`.
fn has_nested_track_array(elem: &Value) -> bool {
    let Value::Object(map) = elem else {
        return false;
    };
    map.values().any(|v| match v {
        Value::Array(inner) => {
            let shaped = inner.iter().take(16).filter(|e| is_track_shaped(e)).count();
            shaped >= 1 && shaped * 2 >= inner.len().min(16)
        }
        _ => false,
    })
}

/// THE SAME BOUNDED PROBE EXTRACTION USES. Detection and extraction must not
/// drift: an element that passes here and yields nothing there would inflate
/// the candidate score against a real list.
fn is_track_shaped(elem: &Value) -> bool {
    let target = unwrap_element(elem);
    let Value::Object(_) = target else {
        return false;
    };
    harvest_str(target, TITLE_KEYS, ELEM_FIELD_DEPTH).is_some()
        && harvest_artist(target, ELEM_FIELD_DEPTH).is_some()
}

// ---------------------------------------------------------------------------
// (b) Field extraction — synonym harvest
// ---------------------------------------------------------------------------

/// Unwrap ONE object-valued wrapper key (`{track:{...}}` -> the inner object).
/// A SCALAR under the same key is a title, not a wrapper, so it never unwraps.
fn unwrap_element(elem: &Value) -> &Value {
    let Value::Object(map) = elem else {
        return elem;
    };
    for (k, v) in map.iter() {
        if v.is_object() && WRAPPER_KEYS.contains(&norm_key(k).as_str()) {
            return v;
        }
    }
    elem
}

fn extract_track(elem: &Value) -> Option<ImportTrack> {
    let t = unwrap_element(elem);
    let title = harvest_str(t, TITLE_KEYS, ELEM_FIELD_DEPTH)?;
    let title = title.trim().to_string();
    // KEEP RULE: title required, artist optional.
    //
    // The matcher's arithmetic is why. A title-less row maxes at artist 0.3 +
    // album 0.1 = 0.4, below the 0.65 threshold — mathematically unmatchable,
    // so dropping it loses nothing and keeps the preview count honest. A
    // title-ONLY row maxes at 0.6 and clears only on an exact normalized hit
    // plus the duration bonus; it is kept, and if it misses it surfaces in
    // `skipped_tracks` where the user can see it.
    if title.is_empty() {
        return None;
    }
    Some(ImportTrack {
        title,
        artist: harvest_artist(t, ELEM_FIELD_DEPTH).unwrap_or_default(),
        album: harvest_str(t, ALBUM_KEYS, ELEM_FIELD_DEPTH).filter(|s| !s.trim().is_empty()),
        duration_ms: harvest_duration(t, ELEM_FIELD_DEPTH),
        isrc: harvest_str(t, &["isrc"], ELEM_FIELD_DEPTH)
            .and_then(|s| super::file::decode::normalize_isrc(&s)),
        provider_id: None,
        provider_url: None,
    })
}

/// First non-empty scalar under any of `keys`, within `depth` object descents.
/// Direct fields win over nested ones (breadth before depth).
fn harvest_str(node: &Value, keys: &[&str], depth: usize) -> Option<String> {
    let Value::Object(map) = node else {
        return None;
    };
    for (k, v) in map.iter() {
        if !keys.contains(&norm_key(k).as_str()) {
            continue;
        }
        if let Some(s) = as_non_empty_str(v) {
            return Some(s);
        }
        // The synonym matched but its value is an OBJECT — `album:{name:"Al"}`,
        // which is the Spotify/Apple shape and by far the commonest way an
        // album arrives. Read the name out of it rather than walking past a
        // key that already told us it is the right one.
        if v.is_object() {
            if let Some(s) = harvest_str(v, &["name", "title"], 1) {
                return Some(s);
            }
        }
    }
    if depth == 0 {
        return None;
    }
    for (_k, v) in map.iter() {
        if v.is_object() {
            if let Some(s) = harvest_str(v, keys, depth - 1) {
                return Some(s);
            }
        }
    }
    None
}

fn as_non_empty_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Artists come in four shapes: `"A"`, `["A","B"]`, `[{name:"A"}]`, `{name:"A"}`.
fn harvest_artist(node: &Value, depth: usize) -> Option<String> {
    let Value::Object(map) = node else {
        return None;
    };
    for (k, v) in map.iter() {
        if !ARTIST_KEYS.contains(&norm_key(k).as_str()) {
            continue;
        }
        if let Some(s) = artist_value(v) {
            return Some(s);
        }
    }
    if depth == 0 {
        return None;
    }
    for (_k, v) in map.iter() {
        if v.is_object() {
            if let Some(s) = harvest_artist(v, depth - 1) {
                return Some(s);
            }
        }
    }
    None
}

fn artist_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Object(_) => harvest_str(v, &["name", "artistname", "title"], 1),
        Value::Array(items) => {
            let names: Vec<String> = items
                .iter()
                .filter_map(|i| match i {
                    Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
                    Value::Object(_) => harvest_str(i, &["name", "artistname", "title"], 1),
                    _ => None,
                })
                .collect();
            if names.is_empty() {
                None
            } else {
                Some(names.join(", "))
            }
        }
        _ => None,
    }
}

/// Duration in ms.
///
/// TRUST THE KEY NAME FIRST. `duration_ms` is milliseconds and `runtime` is
/// seconds, and both are unambiguous. Only a genuinely unit-less key
/// (`duration`, `time`) falls back to the magnitude rule.
///
/// The magnitude rule's known blind spots, stated rather than hidden: a track
/// longer than 10 000 s expressed in seconds (a DJ mix, an audiobook chapter)
/// reads as ~10 s of ms, and a sub-10 s value in ms reads as seconds. The
/// damage is bounded — duration is a ±0.05 bonus in the matcher and never a
/// gate — but a wrongly-"close" value can hand out a spurious bonus, which is
/// why the key name wins whenever there is one.
fn harvest_duration(node: &Value, depth: usize) -> Option<u64> {
    let Value::Object(map) = node else {
        return None;
    };
    for (k, v) in map.iter() {
        let nk = norm_key(k);
        if !DURATION_KEYS.contains(&nk.as_str()) {
            continue;
        }
        let Some(n) = as_f64(v) else { continue };
        if n <= 0.0 {
            continue;
        }
        let ms = match nk.as_str() {
            "durationms" => n,
            "length" | "runtime" | "durationseconds" => n * 1000.0,
            // Ambiguous: `duration`, `time`.
            _ => {
                if n >= 10_000.0 {
                    n
                } else {
                    n * 1000.0
                }
            }
        };
        return Some(ms.round() as u64);
    }
    if depth == 0 {
        return None;
    }
    for (_k, v) in map.iter() {
        if v.is_object() {
            if let Some(d) = harvest_duration(v, depth - 1) {
                return Some(d);
            }
        }
    }
    None
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// A `name`/`title`/`playlistName` scalar directly on this object.
fn scalar_name(node: &Value) -> Option<String> {
    let Value::Object(map) = node else {
        return None;
    };
    for (k, v) in map.iter() {
        if PLAYLIST_NAME_KEYS.contains(&norm_key(k).as_str()) {
            if let Value::String(s) = v {
                let s = s.trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(json: &str) -> Result<ImportPlaylist, PlaylistImportError> {
        parse(json.as_bytes(), "export.json")
    }

    #[test]
    fn a_flat_array_of_tracks() {
        let doc = r#"[{"title":"A","artist":"X"},{"title":"B","artist":"Y"}]"#;
        let r = p(doc).unwrap();
        assert_eq!(r.tracks.len(), 2);
        assert_eq!(r.tracks[0].title, "A");
        assert_eq!(r.name, "export"); // the filename stem
    }

    #[test]
    fn a_named_wrapper_object() {
        let doc = r#"{"name":"My Mix","tracks":[{"name":"A","artist":"X"}]}"#;
        let r = p(doc).unwrap();
        assert_eq!(r.name, "My Mix");
        assert_eq!(r.tracks.len(), 1);
    }

    #[test]
    fn the_spotify_shape_unwraps_its_track_key() {
        let doc = r#"{"name":"Liked","tracks":{"items":[
            {"added_at":"2026","track":{"name":"A","artists":[{"name":"X"},{"name":"Y"}],
             "album":{"name":"Al"},"duration_ms":211000}}
        ]}}"#;
        let r = p(doc).unwrap();
        assert_eq!(r.tracks.len(), 1);
        assert_eq!(r.tracks[0].title, "A");
        assert_eq!(r.tracks[0].artist, "X, Y");
        assert_eq!(r.tracks[0].album.as_deref(), Some("Al"));
        assert_eq!(r.tracks[0].duration_ms, Some(211_000));
    }

    #[test]
    fn a_deep_graphql_envelope_is_found_by_shape() {
        // Depth 3 from the root — the case a hard two-level cap would drop.
        let doc = r#"{"data":{"playlist":{"title":"Deep","items":[
            {"title":"A","artist":"X"},{"title":"B","artist":"Y"}]}}}"#;
        let r = p(doc).unwrap();
        assert_eq!(r.tracks.len(), 2);
        assert_eq!(r.name, "Deep");
    }

    #[test]
    fn a_sibling_artists_array_does_not_win() {
        // `artists` has a title-like `name` but no artist-like key -> rejected.
        let doc = r#"{"artists":[{"name":"X"},{"name":"Y"},{"name":"Z"}],
                      "tracks":[{"title":"A","artist":"X"}]}"#;
        let r = p(doc).unwrap();
        assert_eq!(r.tracks.len(), 1);
        assert_eq!(r.tracks[0].title, "A");
    }

    #[test]
    fn an_album_array_is_demoted_and_the_real_tracks_win() {
        // The discography shape: each element has BOTH a title and an artist,
        // which is exactly what would fool a naive both-fields test.
        let doc = r#"{"albums":[
            {"title":"Album One","artist":"X","tracks":[
                {"title":"T1","artist":"X"},{"title":"T2","artist":"X"}]},
            {"title":"Album Two","artist":"X","tracks":[
                {"title":"T3","artist":"X"}]}
        ]}"#;
        let r = p(doc).unwrap();
        let titles: Vec<&str> = r.tracks.iter().map(|t| t.title.as_str()).collect();
        // The album NAMES must not be the import.
        assert!(!titles.contains(&"Album One"));
        assert!(titles.contains(&"T1"));
    }

    #[test]
    fn a_scalar_track_key_is_a_title_and_an_object_one_is_a_wrapper() {
        let scalar = r#"[{"track":"Song Name","artist":"X"}]"#;
        assert_eq!(p(scalar).unwrap().tracks[0].title, "Song Name");
        let object = r#"[{"track":{"title":"Inner","artist":"X"}}]"#;
        assert_eq!(p(object).unwrap().tracks[0].title, "Inner");
    }

    #[test]
    fn duration_trusts_the_key_name_before_the_magnitude() {
        // Explicit ms.
        let ms = r#"[{"title":"A","artist":"X","duration_ms":180000}]"#;
        assert_eq!(p(ms).unwrap().tracks[0].duration_ms, Some(180_000));
        // Explicitly seconds, even at a magnitude the fallback would call ms.
        let secs = r#"[{"title":"A","artist":"X","runtime":12000}]"#;
        assert_eq!(p(secs).unwrap().tracks[0].duration_ms, Some(12_000_000));
        // Ambiguous + small -> seconds (the Deezer shape).
        let amb_s = r#"[{"title":"A","artist":"X","duration":200}]"#;
        assert_eq!(p(amb_s).unwrap().tracks[0].duration_ms, Some(200_000));
        // Ambiguous + large -> already ms.
        let amb_ms = r#"[{"title":"A","artist":"X","duration":200000}]"#;
        assert_eq!(p(amb_ms).unwrap().tracks[0].duration_ms, Some(200_000));
        // Numeric strings parse.
        let str_n = r#"[{"title":"A","artist":"X","duration":"200"}]"#;
        assert_eq!(p(str_n).unwrap().tracks[0].duration_ms, Some(200_000));
    }

    #[test]
    fn isrc_is_normalized_to_the_bare_form() {
        let doc = r#"[{"title":"A","artist":"X","isrc":"US-KO1-16-00123"}]"#;
        assert_eq!(p(doc).unwrap().tracks[0].isrc.as_deref(), Some("USKO11600123"));
        // Junk in an isrc field is dropped, not passed through — it would
        // otherwise misfire the matcher's score-1.0 short circuit.
        let junk = r#"[{"title":"A","artist":"X","isrc":"not-an-isrc"}]"#;
        assert_eq!(p(junk).unwrap().tracks[0].isrc, None);
    }

    #[test]
    fn a_title_less_row_is_dropped_and_a_title_only_row_is_kept() {
        // Both rows must be present for the array to qualify, so give the
        // second one an artist and no title.
        let doc = r#"{"tracks":[{"title":"Kept","artist":"X"},{"artist":"Y"}]}"#;
        let r = p(doc).unwrap();
        assert_eq!(r.tracks.len(), 1);
        assert_eq!(r.tracks[0].title, "Kept");
    }

    #[test]
    fn no_track_shaped_array_fails_loud() {
        let doc = r#"{"settings":{"volume":80},"paths":["/a","/b"]}"#;
        assert!(matches!(p(doc), Err(PlaylistImportError::JsonShapeUnrecognized)));
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        assert!(matches!(p("{nope"), Err(PlaylistImportError::Parse(_))));
    }

    #[test]
    fn oversize_is_refused_before_the_parse() {
        let big = vec![b'a'; super::super::MAX_IMPORT_BYTES + 1];
        assert!(matches!(
            parse(&big, "x.json"),
            Err(PlaylistImportError::FileTooLarge)
        ));
    }

    #[test]
    fn the_output_is_capped() {
        let mut items = String::from("[");
        for i in 0..(MAX_TRACKS + 50) {
            if i > 0 {
                items.push(',');
            }
            items.push_str(&format!(r#"{{"title":"T{i}","artist":"X"}}"#));
        }
        items.push(']');
        let r = p(&items).unwrap();
        assert_eq!(r.tracks.len(), MAX_TRACKS);
    }

    #[test]
    fn the_playlist_name_never_comes_from_inside_the_array() {
        // Every element has a `name`; the playlist name must not be "A".
        let doc = r#"[{"name":"A","artist":"X"},{"name":"B","artist":"Y"}]"#;
        let r = p(doc).unwrap();
        assert_eq!(r.name, "export");
    }
}
