//! Discography Builder controller — Slint-free port of
//! `crates/qbz/src/myqbz_builder.rs` (956 lines). It fetches an artist's
//! complete discography from THREE sources (Qobuz artist page + local library +
//! Plex cache), dedupes the releases into GROUPS (one logical album = a chosen
//! "primary" + "alternates"), and persists the user-selected groups as a
//! `kind='artist_collection'` collection (`source_type='artist_discography'`,
//! `source_ref=<artist_id>`).
//!
//! `artist_discography` is a MARKER, not a sync feature: the builder resolves
//! the artist's albums and bulk-adds them as ordinary album items via
//! `qbz_mixtape::repo` — there is NO sync command. On save it collapses Plex
//! candidates to `source='local'` (the LocalLibrary resolver re-detects Plex at
//! enqueue time) and navigates to the new collection's detail.
//!
//! Sourcing notes (degradation), 1:1 with the reference:
//! - Qobuz: `runtime.core().get_artist_page` — the full path, sets artist name
//!   + avatar url. Runs FIRST because the local half needs the resolved name.
//! - Local + Plex: ONE unified `get_albums_metadata_page(…, plex_cache_db_path)`
//!   query (the same one the Local Library Albums tab uses,
//!   `local_albums.rs:31-61`). Plex rows arrive in that set with
//!   `source == "plex"` when the Plex master toggle is ON. Rows are filtered to
//!   the artist by a case-insensitive match on `artist` OR any comma-split entry
//!   in `all_artists`. If the DB is unavailable the local/plex contribution
//!   degrades to empty (logged by `with_db`) — the Qobuz path still fully works.
//! - Plex cold-start: ONE 2-second retry when Plex is enabled and the first
//!   fetch produced no `plex` row (`qbz/src/main.rs:2889-2905`).
//!
//! PORT NOTES (divergences from the reference, all deliberate):
//! - Network-folder exclusion uses the shared live offline/connectivity gate,
//!   so the builder sees the same candidate set as Local Library.
//! - The reference DECODES the avatar into a `slint::Image`; here the avatar is
//!   a `file://` path resolved through `artwork_qt` (`artistAvatarPath`).
//! - Candidate covers are NOT downloaded: no `DiscoCandidateRow` cell renders
//!   artwork (spec 01 §8.5) — the url only rides into the saved collection item.
//!   `artPath` is resolved once at install from the on-disk cache and cached on
//!   the candidate, so a checkbox toggle re-publishes with zero I/O.
//! - No offline early-return: the reference has none either. Offline simply
//!   makes `get_artist_page` fail, which lands in `loadError` and renders
//!   verbatim (spec 01 §8.8).
//! - The three strings `Artist Collection` (default name), `Artist Collection
//!   created` and `Failed to create collection` stay RAW ENGLISH LITERALS, 1:1
//!   with the reference (`myqbz_builder.rs:858,936,941`) — they are absent from
//!   the catalogs and translating them would be a fix, not a port
//!   (spec 02 §2.4.2).
//!
//! FILE SPLIT: the two source fetchers live in `myqbz_builder_fetch_qt.rs`
//! (this file crossed the recipe's mandatory ~1200-line split line, §T3). The
//! classifier, grouping, selection maths and save flow stay HERE — they are one
//! algorithm and the recipe forbids splitting it.
//!
//! HANDOFF (doc keys): `DiscoDoc` carries spec 02 §5.8 verbatim PLUS two
//! ADDITIVE keys the QML cannot produce itself:
//! - `rows` — the flattened `primary`/`alt` table spec 01 §8.9 requires
//!   ("Flatten the groups into ONE array of row descriptors in Rust"), with the
//!   precomputed `restOpacity`. `groups` is still published unchanged, so a
//!   consumer written against §5.8 alone is unaffected.
//! - `footerCount` — the pre-rendered plural footer line (spec 01 §8.7). QML has
//!   no `tf`/`t_args` invokable (`QbzSession.tr` is the only i18n entry point),
//!   so a plural sentence has to be built here.
//!
//! HANDOFF (gap): spec 01 §8.7 wants `nameTrimmedEmpty` Rust-owned, but the
//! pinned bridge surface (spec 02 §4.3) has NO `setName` invokable — the name
//! only reaches Rust as `create(name)`'s argument. The view therefore has to
//! gate its Create button on a JS `text.trim() === ""`. Not published here.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use cxx_qt_lib::QString;
use qbz_models::mixtape::{AlbumSource, CollectionKind, CollectionSourceType, ItemType};
use serde::Serialize;

use crate::myqbz_builder_fetch_qt::{fetch_local_and_plex, fetch_qobuz};

// ──────────────────────────── candidate model ─────────────────────────────

/// One release candidate (a primary or an alternate). Plain `Send` data built on
/// the worker thread; `publish` maps it into `DiscoCandidateJson`.
#[derive(Clone)]
pub struct Candidate {
    pub group_key: String,
    /// "qobuz" | "local" | "plex".
    pub source: String,
    pub source_item_id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    pub artwork_url: Option<String>,
    /// `file://…` resolved ONCE at install from the artwork cache ("" when the
    /// cover is not on disk). Never re-resolved on a re-publish.
    pub artwork_path: String,
    pub track_count: Option<i32>,
    pub max_bit_depth: Option<u32>,
    /// kHz.
    pub max_sample_rate: Option<f64>,
    pub format: String,
    pub is_compilation: bool,
    /// The AUTO-classified release type (before any user override).
    pub release_type: String,
    pub quality_score: i64,
}

impl Candidate {
    /// `${source}|${source_item_id}` — the dedupe + selection key.
    fn key(&self) -> String {
        format!("{}|{}", self.source, self.source_item_id)
    }
}

/// One dedupe group: a primary plus its alternates.
#[derive(Clone)]
pub struct Group {
    pub key: String,
    pub title: String,
    pub year: Option<i32>,
    pub primary: Candidate,
    pub alternates: Vec<Candidate>,
    pub is_compilation: bool,
}

/// The full builder session state. Process-global (the controller mutates it,
/// `publish` renders it) — the port's `static Mutex<Option<…>>` convention with
/// a always-present value, since an empty session is a valid one.
struct Session {
    artist_id: String,
    artist_name: String,
    artist_avatar_url: String,
    artist_avatar_path: String,
    groups: Vec<Group>,
    /// `Record<groupKey, Set<candidateKey>>`.
    checked: HashMap<String, Vec<String>>,
    order_by: String,
    loading: bool,
    load_error: String,
    creating: bool,
    default_name: String,
    /// Monotonic load id — a late reply for a previous artist is dropped
    /// instead of overwriting the current session (the `artist_qt.rs`
    /// generation guard, adapted).
    generation: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            artist_id: String::new(),
            artist_name: String::new(),
            artist_avatar_url: String::new(),
            artist_avatar_path: String::new(),
            groups: Vec::new(),
            checked: HashMap::new(),
            order_by: "release_date".to_string(),
            loading: false,
            load_error: String::new(),
            creating: false,
            default_name: String::new(),
            generation: 0,
        }
    }
}

static SESSION: LazyLock<Mutex<Session>> = LazyLock::new(|| Mutex::new(Session::default()));

fn session<R>(f: impl FnOnce(&mut Session) -> R) -> R {
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

// ──────────────────────── release-type override store ──────────────────────
//
// A process-global map persisted to a JSON sidecar under the user data dir so a
// chosen override survives the session. NOT per-user, and SHARED with the Slint
// build — same path, same shape (spec 02 open question #12). Keyed
// `${source}|${source_item_id}` -> one of album|ep|single|live|compilation.

static OVERRIDES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(load_overrides()));

fn overrides_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| {
        d.join("qbz")
            .join("discography-release-type-overrides.json")
    })
}

fn load_overrides() -> HashMap<String, String> {
    let Some(path) = overrides_path() else {
        return HashMap::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return HashMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_overrides(map: &HashMap<String, String>) {
    let Some(path) = overrides_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(map) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// A snapshot of the sidecar. The EFFECTIVE release type of a candidate is
/// `snapshot.get(candidate.key())` else its auto-classified `release_type`; the
/// lookup is done in `map_candidate` against ONE snapshot per document rather
/// than a lock per candidate.
fn overrides_snapshot() -> HashMap<String, String> {
    OVERRIDES.lock().map(|m| m.clone()).unwrap_or_default()
}

// ──────────────────────────── classification ──────────────────────────────

/// `normalizeTitle`: lowercase, strip parenthetical/bracket edition suffixes,
/// collapse whitespace, trim. Mirrors the reference regex.
pub(crate) fn normalize_title(title: &str) -> String {
    let lower = title.to_lowercase();
    // Edition keywords that mark a re-issue suffix.
    const KEYWORDS: [&str; 14] = [
        "deluxe",
        "remaster",
        "expanded",
        "anniversary",
        "collector",
        "special",
        "bonus",
        "extended",
        "definitive",
        "20th",
        "25th",
        "30th",
        "40th",
        "50th",
    ];
    // Find the first '(' or '[' whose inner text starts with an edition keyword
    // (or an Nth marker) and strip from there to the end.
    let bytes = lower.as_bytes();
    let mut cut = lower.len();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if ch == '(' || ch == '[' {
            let inner = lower[i + 1..].trim_start();
            let starts_kw = KEYWORDS.iter().any(|k| inner.starts_with(k)) || starts_with_nth(inner);
            if starts_kw {
                cut = i;
                break;
            }
        }
        i += 1;
    }
    let trimmed = &lower[..cut];
    // Collapse whitespace + trim.
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `s` begins with an `<digits>th` token (e.g. "10th anniversary").
fn starts_with_nth(s: &str) -> bool {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    !digits.is_empty() && s[digits.len()..].starts_with("th")
}

/// `isCompilation(title)` — best-of / greatest-hits markers.
pub(crate) fn title_is_compilation(title: &str) -> bool {
    let l = title.to_lowercase();
    const M: [&str; 6] = [
        "best of",
        "greatest hits",
        "anthology",
        "the very best",
        "essential",
        "collection",
    ];
    M.iter().any(|m| l.contains(m))
}

/// `classifyRelease` — precedence: compilation → live → ep → single → album →
/// track-count heuristic. `qobuz_release_type` / `qobuz_group_type` are the
/// per-item and the group's release_type from /artist/page (None for local/plex).
pub(crate) fn classify_release(
    title: &str,
    track_count: Option<i32>,
    qobuz_release_type: Option<&str>,
    qobuz_group_type: Option<&str>,
    title_is_comp: bool,
) -> String {
    let l = title.to_lowercase();
    let rt = qobuz_release_type.unwrap_or("");
    let gt = qobuz_group_type.unwrap_or("");
    let type_is = |needle: &str| rt.eq_ignore_ascii_case(needle) || gt.eq_ignore_ascii_case(needle);

    if rt.to_lowercase().contains("compilation")
        || gt.to_lowercase().contains("compilation")
        || title_is_comp
    {
        return "compilation".to_string();
    }
    let word = |w: &str| {
        // crude word-boundary contains
        l.split(|c: char| !c.is_alphanumeric()).any(|tok| tok == w)
    };
    if type_is("live") || word("live") || word("concert") || word("unplugged") {
        return "live".to_string();
    }
    if type_is("ep") || word("ep") {
        return "ep".to_string();
    }
    if type_is("single") {
        return "single".to_string();
    }
    if type_is("album") {
        return "album".to_string();
    }
    match track_count {
        Some(n) if n <= 3 => "single".to_string(),
        Some(n) if n <= 6 => "ep".to_string(),
        _ => "album".to_string(),
    }
}

/// `qualityScore`: bit*10_000_000 + rateHz + fmtBonus.
pub(crate) fn quality_score(
    max_bit_depth: Option<u32>,
    max_sample_rate_khz: Option<f64>,
    format: &str,
) -> i64 {
    let bit = max_bit_depth.unwrap_or(16) as i64;
    let rate_hz = ((max_sample_rate_khz.unwrap_or(44.1)) * 1000.0).round() as i64;
    let fmt = format.to_lowercase();
    let fmt_bonus = if fmt.contains("flac") || fmt.contains("alac") {
        1000
    } else if fmt.contains("mp3") || fmt.contains("aac") {
        0
    } else {
        500
    };
    bit * 10_000_000 + rate_hz + fmt_bonus
}

// ─────────────────────────── quality badge (tier) ──────────────────────────
//
// The reference calls `crate::quality::badge(format, bit, rate)`. So does this
// now: the four helpers that used to live inline here were hoisted VERBATIM
// into `crate::quality_qt` — the named follow-up this block's own note asked
// for, done in its own commit as that note required. Local Library badged DSD
// as CD for want of exactly these.
use crate::quality_qt::badge;

// ──────────────────────────── buildGroups ─────────────────────────────────

/// Dedupe incoming candidates by `${source}|${source_item_id}`, bucket by
/// `group_key`, sort each bucket by quality (Qobuz wins ties for the primary
/// slot), and produce groups.
pub fn build_groups(candidates: Vec<Candidate>) -> Vec<Group> {
    // 1. Dedupe by candidate key, preserving first-seen order.
    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<Candidate> = Vec::new();
    for c in candidates {
        if seen.insert(c.key()) {
            deduped.push(c);
        }
    }

    // 2. Bucket by group_key, preserving first-seen bucket order (Map order).
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<Candidate>> = HashMap::new();
    for c in deduped {
        if !buckets.contains_key(&c.group_key) {
            order.push(c.group_key.clone());
        }
        buckets.entry(c.group_key.clone()).or_default().push(c);
    }

    // 3+4. Within each bucket sort by quality_score desc, Qobuz-preferred on
    // ties; primary = sorted[0], alternates = rest.
    let mut groups: Vec<Group> = Vec::new();
    for key in order {
        let mut bucket = buckets.remove(&key).unwrap_or_default();
        if bucket.is_empty() {
            continue;
        }
        bucket.sort_by(|a, b| {
            b.quality_score.cmp(&a.quality_score).then_with(|| {
                // equal quality → qobuz first.
                let aq = a.source == "qobuz";
                let bq = b.source == "qobuz";
                bq.cmp(&aq)
            })
        });
        let is_compilation = bucket.iter().all(|c| c.is_compilation);
        let primary = bucket.remove(0);
        let title = primary.title.clone();
        let year = primary.year;
        groups.push(Group {
            key,
            title,
            year,
            primary,
            alternates: bucket,
            is_compilation,
        });
    }
    groups
}

// ──────────────────────────── sort (orderedGroups) ─────────────────────────

/// Return the groups in the active sort order (clones into a fresh Vec).
fn ordered_groups(groups: &[Group], order_by: &str) -> Vec<Group> {
    let mut g = groups.to_vec();
    match order_by {
        "title" => g.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        "manual" => {} // insertion order — unchanged.
        _ => {
            // release_date: year asc, nulls last, tiebreak by title.
            g.sort_by(|a, b| match (a.year, b.year) {
                (None, None) => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(ya), Some(yb)) => ya.cmp(&yb),
            });
        }
    }
    g
}

// ──────────────────────────── selection helpers ────────────────────────────

fn candidate_key(c: &Candidate) -> String {
    c.key()
}

fn is_checked(s: &Session, group_key: &str, cand_key: &str) -> bool {
    s.checked
        .get(group_key)
        .map(|set| set.iter().any(|k| k == cand_key))
        .unwrap_or(false)
}

/// `allPrimariesChecked` — every non-compilation group has its primary checked
/// (compilations are treated as always-satisfied).
fn all_primaries_checked(s: &Session) -> bool {
    !s.groups.is_empty()
        && s.groups
            .iter()
            .all(|g| g.is_compilation || is_checked(s, &g.key, &candidate_key(&g.primary)))
}

fn some_primaries_checked(s: &Session) -> bool {
    !all_primaries_checked(s)
        && s.groups
            .iter()
            .any(|g| !g.is_compilation && is_checked(s, &g.key, &candidate_key(&g.primary)))
}

/// Total checked candidate count across all groups (primaries + alternates).
fn selected_count(s: &Session) -> usize {
    s.checked.values().map(|set| set.len()).sum()
}

// ──────────────────────────── published document ──────────────────────────

/// One candidate row (spec 02 §5.8).
#[derive(Clone, Default, Serialize)]
pub struct DiscoCandidateJson {
    /// `"{source}|{sourceItemId}"`.
    pub key: String,
    /// `qobuz | local | plex`.
    pub source: String,
    #[serde(rename = "sourceItemId")]
    pub source_item_id: String,
    pub title: String,
    pub artist: String,
    /// `"—"` when absent.
    pub year: String,
    /// `"—"` when absent.
    pub tracks: String,
    #[serde(rename = "artUrl")]
    pub art_url: String,
    #[serde(rename = "artPath")]
    pub art_path: String,
    /// EFFECTIVE type after the override: `album | ep | single | live | compilation`.
    #[serde(rename = "releaseType")]
    pub release_type: String,
    #[serde(rename = "isOverridden")]
    pub is_overridden: bool,
    #[serde(rename = "qualityTier")]
    pub quality_tier: String,
    #[serde(rename = "qualityDetail")]
    pub quality_detail: String,
    pub checked: bool,
}

/// One dedupe group (spec 02 §5.8).
#[derive(Clone, Default, Serialize)]
pub struct DiscoGroupJson {
    pub key: String,
    pub title: String,
    /// `"—"` when absent.
    pub year: String,
    #[serde(rename = "isCompilation")]
    pub is_compilation: bool,
    pub primary: DiscoCandidateJson,
    pub alternates: Vec<DiscoCandidateJson>,
}

/// ONE flattened table row (spec 01 §8.9): the view mounts a single recycled
/// `ListView` over these, never a `Repeater` of groups each holding a
/// `Repeater` of alternates.
#[derive(Clone, Default, Serialize)]
pub struct DiscoRowJson {
    /// `"primary"` | `"alt"`.
    pub kind: String,
    #[serde(rename = "groupKey")]
    pub group_key: String,
    #[serde(rename = "groupIsCompilation")]
    pub group_is_compilation: bool,
    /// Resting opacity: primary = `isCompilation ? 0.65 : 1.0`, alternate =
    /// **0.72 unconditionally** (`DiscographyBuilderView.slint:713,720`).
    #[serde(rename = "restOpacity")]
    pub rest_opacity: f64,
    pub candidate: DiscoCandidateJson,
}

/// `QbzDisco.discoJson` (spec 02 §5.8, plus the two additive keys spec 01
/// §8.7/§8.9 require — see the module HANDOFF note).
#[derive(Clone, Default, Serialize)]
pub struct DiscoDoc {
    #[serde(rename = "artistId")]
    pub artist_id: String,
    #[serde(rename = "artistName")]
    pub artist_name: String,
    #[serde(rename = "artistAvatarUrl")]
    pub artist_avatar_url: String,
    #[serde(rename = "artistAvatarPath")]
    pub artist_avatar_path: String,
    pub loading: bool,
    /// Non-empty → the raw message renders verbatim.
    #[serde(rename = "loadError")]
    pub load_error: String,
    #[serde(rename = "orderBy")]
    pub order_by: String,
    #[serde(rename = "allChecked")]
    pub all_checked: bool,
    #[serde(rename = "someChecked")]
    pub some_checked: bool,
    #[serde(rename = "selectedCount")]
    pub selected_count: i32,
    #[serde(rename = "groupCount")]
    pub group_count: i32,
    pub creating: bool,
    /// `"{artist} — Complete Discography"` (em-dash U+2014), Rust-built so the
    /// `t_args` key is shared with the reference.
    #[serde(rename = "defaultName")]
    pub default_name: String,
    /// Pre-rendered plural footer line (spec 01 §8.7) — QML has no `tf`.
    #[serde(rename = "footerCount")]
    pub footer_count: String,
    /// In the CURRENT sort order.
    pub groups: Vec<DiscoGroupJson>,
    /// The flattened table (spec 01 §8.9), same order as `groups`.
    pub rows: Vec<DiscoRowJson>,
}

/// `overrides` is snapshotted ONCE per document (see `build_doc`): the naive
/// shape locks `OVERRIDES` twice per candidate, and `publish` runs on every
/// checkbox toggle over a table that can hold hundreds of rows.
fn map_candidate(
    s: &Session,
    c: &Candidate,
    overrides: &HashMap<String, String>,
) -> DiscoCandidateJson {
    let (tier, detail) = badge(&c.format, c.max_bit_depth, c.max_sample_rate);
    let over = overrides.get(&c.key());
    DiscoCandidateJson {
        key: candidate_key(c),
        source: c.source.clone(),
        source_item_id: c.source_item_id.clone(),
        title: c.title.clone(),
        artist: c.artist.clone(),
        year: c
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "—".to_string()),
        tracks: c
            .track_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string()),
        art_url: c.artwork_url.clone().unwrap_or_default(),
        art_path: c.artwork_path.clone(),
        release_type: over.cloned().unwrap_or_else(|| c.release_type.clone()),
        is_overridden: over.is_some(),
        quality_tier: tier,
        quality_detail: detail,
        checked: is_checked(s, &c.group_key, &candidate_key(c)),
    }
}

/// Build the document from the session (caller holds the lock).
fn build_doc(s: &Session) -> DiscoDoc {
    let overrides = overrides_snapshot();
    let ordered = ordered_groups(&s.groups, &s.order_by);
    let mut groups: Vec<DiscoGroupJson> = Vec::with_capacity(ordered.len());
    let mut rows: Vec<DiscoRowJson> = Vec::with_capacity(ordered.len() * 2);
    for g in &ordered {
        let primary = map_candidate(s, &g.primary, &overrides);
        let alternates: Vec<DiscoCandidateJson> = g
            .alternates
            .iter()
            .map(|c| map_candidate(s, c, &overrides))
            .collect();
        rows.push(DiscoRowJson {
            kind: "primary".to_string(),
            group_key: g.key.clone(),
            group_is_compilation: g.is_compilation,
            rest_opacity: if g.is_compilation { 0.65 } else { 1.0 },
            candidate: primary.clone(),
        });
        for alt in &alternates {
            rows.push(DiscoRowJson {
                kind: "alt".to_string(),
                group_key: g.key.clone(),
                group_is_compilation: g.is_compilation,
                // An alternate inside a compilation group is still 0.72.
                rest_opacity: 0.72,
                candidate: alt.clone(),
            });
        }
        groups.push(DiscoGroupJson {
            key: g.key.clone(),
            title: g.title.clone(),
            year: g
                .year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "—".to_string()),
            is_compilation: g.is_compilation,
            primary,
            alternates,
        });
    }
    let selected = selected_count(s) as i64;
    let group_count = s.groups.len() as i64;
    let selected_str = selected.to_string();
    let group_count_str = group_count.to_string();
    let footer_count = qbz_i18n::tf(
        "{} of {} album selected",
        "{} of {} albums selected",
        group_count,
        &[selected_str.as_str(), group_count_str.as_str()],
    );
    DiscoDoc {
        artist_id: s.artist_id.clone(),
        artist_name: s.artist_name.clone(),
        artist_avatar_url: s.artist_avatar_url.clone(),
        artist_avatar_path: s.artist_avatar_path.clone(),
        loading: s.loading,
        load_error: s.load_error.clone(),
        order_by: s.order_by.clone(),
        all_checked: all_primaries_checked(s),
        some_checked: some_primaries_checked(s),
        selected_count: selected as i32,
        group_count: group_count as i32,
        creating: s.creating,
        default_name: s.default_name.clone(),
        footer_count,
        groups,
        rows,
    }
}

/// Serialize + hop onto the Qt thread. Cheap enough to call on every toggle:
/// no I/O (cover paths are cached on the candidate at install time).
fn publish() {
    let json = session(|s| {
        serde_json::to_string(&build_doc(s)).unwrap_or_else(|e| {
            log::warn!("[qbz-qt] disco doc serialize failed: {e}");
            "{}".to_string()
        })
    });
    crate::disco_bridge::ui(move |mut b| {
        b.as_mut().set_disco_json(QString::from(json.as_str()));
    });
}

/// Re-publish with the current locale's strings (`footerCount`, `defaultName`).
/// Called from `apply_language`.
pub fn republish() {
    publish();
}

/// Drop the session (logout / user switch).
pub fn teardown() {
    session(|s| *s = Session::default());
    publish();
}

// ──────────────────────────── load flow ───────────────────────────────────

/// Reset the builder state before a fresh load. Returns the new generation.
fn reset(artist_id: &str) -> u64 {
    session(|s| {
        let generation = s.generation + 1;
        *s = Session {
            artist_id: artist_id.to_string(),
            loading: true,
            generation,
            ..Session::default()
        };
        generation
    })
}

/// `file://…` for a cover ALREADY on disk, "" otherwise. Synchronous cache
/// lookup only — the builder never downloads candidate covers (no row cell
/// renders one; the url only rides into the saved collection item).
fn resolve_art(url: Option<&str>) -> String {
    match url {
        Some(u) if !u.is_empty() => crate::artwork_qt::cached_path(u),
        _ => String::new(),
    }
}

/// Install the fetched + grouped data and seed the default selection (each
/// non-compilation primary pre-checked). Ignored when `generation` is stale.
fn install(generation: u64, artist_name: String, artist_avatar_url: String, groups: Vec<Group>) {
    let installed = session(|s| {
        if s.generation != generation {
            return false;
        }
        s.artist_name = artist_name.clone();
        s.artist_avatar_url = artist_avatar_url.clone();
        // Avatar + cover paths resolve ONCE here, from the on-disk cache only
        // (T17: no per-row republish, and a toggle must never touch the disk).
        s.artist_avatar_path = if artist_avatar_url.is_empty() {
            String::new()
        } else {
            crate::artwork_qt::cached_path(&artist_avatar_url)
        };
        s.groups = groups;
        for g in s.groups.iter_mut() {
            g.primary.artwork_path = resolve_art(g.primary.artwork_url.as_deref());
            for alt in g.alternates.iter_mut() {
                alt.artwork_path = resolve_art(alt.artwork_url.as_deref());
            }
        }
        // Default selection: non-compilation primaries pre-checked.
        s.checked.clear();
        let snapshot: Vec<(String, bool, String)> = s
            .groups
            .iter()
            .map(|g| (g.key.clone(), g.is_compilation, candidate_key(&g.primary)))
            .collect();
        for (key, is_comp, primary_key) in snapshot {
            let mut set = Vec::new();
            if !is_comp {
                set.push(primary_key);
            }
            s.checked.insert(key, set);
        }
        // Default collection name (em-dash U+2014); `Artist` is the fallback
        // base when the artist name did not resolve.
        let base = if artist_name.is_empty() {
            qbz_i18n::t("Artist")
        } else {
            artist_name.clone()
        };
        s.default_name = qbz_i18n::t_args("{} — Complete Discography", &[base.as_str()]);
        s.loading = false;
        s.load_error = String::new();
        true
    });
    if installed {
        publish();
    }
}

/// Mark the load failed with a raw error message (shown verbatim).
fn fail(generation: u64, message: String) {
    let failed = session(|s| {
        if s.generation != generation {
            return false;
        }
        s.loading = false;
        s.load_error = message;
        true
    });
    if failed {
        publish();
    }
}

/// Open the Discography Builder for `artist_id`. Fetches the artist's releases
/// from Qobuz (sets name + avatar url), then local + Plex by that name
/// (sequential — parallelizing drops local matches against an empty name),
/// dedupes into groups and installs the default selection. Plex gets a single
/// 2-second cold-start retry when enabled and the first fetch returns nothing.
pub fn open(artist_id: String) {
    let generation = reset(&artist_id);
    // PUBLISH BEFORE the route switch, 1:1 with the reference
    // (`main.rs:2874-2876`: `myqbz_builder::reset(&w, …)` then
    // `set_view(ContentView::DiscographyBuilder)`). Both hops are QUEUED onto the
    // Qt thread in FIFO order, so recording first would let the view mount on the
    // PREVIOUS artist's still-published document for a frame.
    publish();
    crate::nav_qt::record("discobuilder");

    let runtime = crate::app();
    crate::spawn(async move {
        // 1. Qobuz first — sets artist name + avatar URL (side effect).
        match fetch_qobuz(&runtime, &artist_id).await {
            Ok((qobuz, artist_name, avatar_url)) => {
                // 2. Local + Plex by the resolved name (sequential, mandatory).
                let name_for_local = artist_name.clone();
                let mut local =
                    tokio::task::spawn_blocking(move || fetch_local_and_plex(&name_for_local))
                        .await
                        .unwrap_or_default();

                // 2b. Plex cold-start retry: if Plex is enabled and we got
                //     nothing from the Plex source, wait 2s and refetch once.
                if crate::local_plex::is_configured() && !local.iter().any(|c| c.source == "plex") {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let name_retry = artist_name.clone();
                    let retried =
                        tokio::task::spawn_blocking(move || fetch_local_and_plex(&name_retry))
                            .await
                            .unwrap_or_default();
                    if retried.iter().any(|c| c.source == "plex") {
                        local = retried;
                    }
                }

                // 3. Merge + group (Qobuz first so it wins primary ties).
                let mut all = qobuz;
                all.extend(local);
                let groups = build_groups(all);
                install(generation, artist_name, avatar_url.clone(), groups);

                // 4. Fetch the avatar if it was not already cached, then ONE
                //    republish (T17 — never one per image).
                if !avatar_url.is_empty() && crate::artwork_qt::cached_path(&avatar_url).is_empty()
                {
                    crate::artwork_qt::download_missing(vec![avatar_url.clone()]).await;
                    let path = crate::artwork_qt::cached_path(&avatar_url);
                    if !path.is_empty() {
                        let changed = session(|s| {
                            if s.generation != generation {
                                return false;
                            }
                            s.artist_avatar_path = path;
                            true
                        });
                        if changed {
                            publish();
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("[qbz-qt] discography builder load failed: {e}");
                fail(generation, e);
            }
        }
    });
}

// ──────────────────────────── interactions ────────────────────────────────

/// Shell back (the footer Cancel — the view has no back button of its own).
pub fn back() {
    crate::nav_qt::back();
}

/// Toggle one candidate's checked state.
pub fn toggle_checked(group_key: &str, cand_key: &str) {
    session(|s| {
        let set = s.checked.entry(group_key.to_string()).or_default();
        if let Some(pos) = set.iter().position(|k| k == cand_key) {
            set.remove(pos);
        } else {
            set.push(cand_key.to_string());
        }
    });
    publish();
}

/// Header tri-state toggle — only ever touches non-compilation PRIMARIES;
/// alternates + compilations are left exactly as the user had them.
pub fn toggle_all() {
    session(|s| {
        let target = !all_primaries_checked(s);
        let prims: Vec<(String, String)> = s
            .groups
            .iter()
            .filter(|g| !g.is_compilation)
            .map(|g| (g.key.clone(), candidate_key(&g.primary)))
            .collect();
        for (gkey, pkey) in prims {
            let set = s.checked.entry(gkey).or_default();
            let has = set.iter().any(|k| k == &pkey);
            if target && !has {
                set.push(pkey);
            } else if !target && has {
                set.retain(|k| k != &pkey);
            }
        }
    });
    publish();
}

/// Change the sort order (`release_date | title | manual`).
pub fn set_order(order_by: &str) {
    session(|s| s.order_by = order_by.to_string());
    publish();
}

/// Open an album from a candidate row. `source` is unused — a blank id is
/// ignored and `crate::open_album` routes local group keys itself.
pub fn open_album(_source: &str, source_item_id: &str) {
    if source_item_id.trim().is_empty() {
        return;
    }
    crate::open_album(source_item_id.to_string());
}

/// Apply a release-type override (persisted) + re-publish.
pub fn set_type_override(source: &str, source_item_id: &str, choice: &str) {
    if let Ok(mut m) = OVERRIDES.lock() {
        m.insert(format!("{source}|{source_item_id}"), choice.to_string());
        save_overrides(&m);
    }
    publish();
}

/// Clear a release-type override + re-publish.
pub fn reset_type_override(source: &str, source_item_id: &str) {
    if let Ok(mut m) = OVERRIDES.lock() {
        m.remove(&format!("{source}|{source_item_id}"));
        save_overrides(&m);
    }
    publish();
}

// ──────────────────────────── save flow ───────────────────────────────────

/// The checked candidates in save order, plus the collection name.
pub struct SavePayload {
    pub name: String,
    pub artist_id: String,
    /// Checked candidates, in save order.
    pub items: Vec<Candidate>,
}

/// Snapshot the current selection into a `SavePayload`, in the CURRENT sort
/// order, each group emitting `[primary, ...alternates]`. `None` when nothing
/// is checked (no create). The name is trimmed; trimmed-empty falls back to the
/// RAW literal `"Artist Collection"` (untranslated, 1:1).
pub fn save_payload(name_raw: &str) -> Option<SavePayload> {
    session(|s| {
        let name = {
            let t = name_raw.trim();
            if t.is_empty() {
                "Artist Collection".to_string()
            } else {
                t.to_string()
            }
        };
        let groups = ordered_groups(&s.groups, &s.order_by);
        let mut items: Vec<Candidate> = Vec::new();
        for g in &groups {
            let mut ordered = vec![g.primary.clone()];
            ordered.extend(g.alternates.iter().cloned());
            for c in ordered {
                if is_checked(s, &c.group_key, &candidate_key(&c)) {
                    items.push(c);
                }
            }
        }
        if items.is_empty() {
            return None;
        }
        Some(SavePayload {
            name,
            artist_id: s.artist_id.clone(),
            items,
        })
    })
}

/// Create the artist_collection + bulk-add the checked candidates. BLOCKING
/// (DB). Returns the new collection id on success. Plex candidates are stored as
/// `source='local'` (the LocalLibrary resolver re-detects Plex at enqueue time
/// via the `plex:` prefix). Per-item errors are logged, the batch continues.
pub fn create_collection(payload: &SavePayload) -> Option<String> {
    // The create-capable accessor: a fresh account has no library.db yet, and
    // `local_state::with_db` would return None (spec 02 §2.1 note / W9).
    crate::library_db_qt::with_db(true, |db| {
        db.with_connection(|conn| {
            let col = qbz_mixtape::repo::create_collection(
                conn,
                CollectionKind::ArtistCollection,
                &payload.name,
                None,
                CollectionSourceType::ArtistDiscography,
                Some(payload.artist_id.as_str()),
            )?;
            for c in &payload.items {
                // Source collapse on save: qobuz -> Qobuz; local OR plex ->
                // Local (the LocalLibrary resolver re-detects Plex at enqueue).
                let src = if c.source == "qobuz" {
                    AlbumSource::Qobuz
                } else {
                    AlbumSource::Local
                };
                if let Err(e) = qbz_mixtape::repo::add_item(
                    conn,
                    &col.id,
                    ItemType::Album,
                    src,
                    &c.source_item_id,
                    &c.title,
                    Some(c.artist.as_str()),
                    c.artwork_url.as_deref(),
                    c.year,
                    c.track_count,
                ) {
                    log::warn!("[qbz-qt] disco builder add_item failed: {e}");
                }
            }
            Ok::<String, rusqlite::Error>(col.id)
        })
        .map_err(|e| {
            qbz_library::LibraryError::Database(format!("disco builder create failed: {e}"))
        })
    })
}

fn set_creating(creating: bool) {
    session(|s| s.creating = creating);
    publish();
}

/// Create the collection from the current selection and navigate to its detail.
/// `name` is whatever the view's NAME field holds (the doc's `defaultName` is
/// what it was seeded with).
pub fn create(name: String) {
    if session(|s| s.creating) {
        return;
    }
    let Some(payload) = save_payload(&name) else {
        return;
    };
    set_creating(true);
    crate::spawn(async move {
        let result = tokio::task::spawn_blocking(move || create_collection(&payload))
            .await
            .ok()
            .flatten();
        set_creating(false);
        match result {
            Some(collection_id) => {
                // Both literals are UNTRANSLATED, 1:1 with the reference.
                crate::toast_qt::success("Artist Collection created");
                // The new artist_collection has to appear in the Collections
                // grid the moment BACK lands there (spec 02 §7 T4).
                crate::myqbz_qt::reload_grids();
                crate::myqbz_open_detail(collection_id);
            }
            None => crate::toast_qt::error("Failed to create collection"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(source: &str, id: &str, title: &str, year: Option<i32>, score: i64) -> Candidate {
        Candidate {
            group_key: format!(
                "{}|{}",
                normalize_title(title),
                year.map(|y| y.to_string()).unwrap_or_default()
            ),
            source: source.to_string(),
            source_item_id: id.to_string(),
            title: title.to_string(),
            artist: "A".to_string(),
            year,
            artwork_url: None,
            artwork_path: String::new(),
            track_count: Some(10),
            max_bit_depth: Some(16),
            max_sample_rate: Some(44.1),
            format: "FLAC".to_string(),
            is_compilation: title_is_compilation(title),
            release_type: "album".to_string(),
            quality_score: score,
        }
    }

    #[test]
    fn normalize_title_strips_edition_suffixes() {
        assert_eq!(
            normalize_title("OK Computer (Deluxe Edition)"),
            "ok computer"
        );
        assert_eq!(normalize_title("Nevermind [Remastered]"), "nevermind");
        assert_eq!(normalize_title("Grace  (20th Anniversary)"), "grace");
        assert_eq!(normalize_title("Kid A (10th Anniversary)"), "kid a");
        // A parenthetical that is NOT an edition marker is preserved.
        assert_eq!(
            normalize_title("Song (Live at Wembley)"),
            "song (live at wembley)"
        );
    }

    /// The sharp edges of `normalize_title` that decide which releases land in
    /// the SAME dedupe bucket. Each of these is a rule the reference has and a
    /// "tidier" rewrite would silently drop.
    #[test]
    fn normalize_title_edge_rules() {
        // The keyword must START the bracket's inner text — a leading article
        // means NO cut (reference scans `inner.starts_with(k)`, not `contains`).
        assert_eq!(
            normalize_title("Album (The Deluxe Edition)"),
            "album (the deluxe edition)"
        );
        // The FIRST *matching* bracket cuts, not the first bracket: a
        // non-edition parenthetical is skipped and the later one still cuts.
        assert_eq!(normalize_title("A (Live) [Remastered]"), "a (live)");
        // Whitespace inside the bracket before the keyword is tolerated.
        assert_eq!(normalize_title("B (  Deluxe )"), "b");
        // A title that OPENS with an edition bracket normalizes to the empty
        // string — every such release for one year buckets together.
        assert_eq!(normalize_title("(Deluxe) Thing"), "");
        // Collapse + trim.
        assert_eq!(normalize_title("  Spaced   Out  "), "spaced out");
        // `<digits>th` marker with no trailing keyword still cuts.
        assert_eq!(normalize_title("C [33rd Party]"), "c [33rd party]");
        assert_eq!(normalize_title("C [33th Party]"), "c");
        // Byte-indexed bracket scan must survive multi-byte titles.
        assert_eq!(normalize_title("Café (Deluxe)"), "café");
        assert_eq!(normalize_title("Café Sessions"), "café sessions");
    }

    /// The compilation marker set is a bare lowercase `contains` over exactly
    /// six needles — deliberately aggressive (it fires inside longer words).
    /// A group whose candidates are ALL compilations starts UNCHECKED, so
    /// adding or dropping a marker changes what a user's collection contains.
    #[test]
    fn title_is_compilation_marker_set() {
        for t in [
            "The Best Of Queen",
            "GREATEST HITS",
            "Anthology 2",
            "The Very Best of Sade",
            "Essential Miles",
            "The Blue Note Collection",
        ] {
            assert!(title_is_compilation(t), "{t} must be a compilation");
        }
        // `contains`, not a word match: substrings inside longer words fire.
        assert!(title_is_compilation("Collections"));
        assert!(title_is_compilation("Essentials"));
        // "best" alone is NOT a marker — the needle is "best of".
        assert!(!title_is_compilation("Best Album Ever"));
        assert!(!title_is_compilation("Greatest"));
        assert!(!title_is_compilation("OK Computer"));
    }

    #[test]
    fn classify_precedence() {
        // compilation wins over everything.
        assert_eq!(
            classify_release("Greatest Hits Live", Some(20), Some("album"), None, true),
            "compilation"
        );
        assert_eq!(
            classify_release("Unplugged", Some(12), None, None, false),
            "live"
        );
        assert_eq!(
            classify_release("Some EP", Some(12), None, None, false),
            "ep"
        );
        assert_eq!(
            classify_release("Whatever", Some(2), None, None, false),
            "single"
        );
        assert_eq!(
            classify_release("Whatever", Some(5), None, None, false),
            "ep"
        );
        assert_eq!(
            classify_release("Whatever", Some(12), None, None, false),
            "album"
        );
        // "epic" must NOT match the `ep` word test.
        assert_eq!(
            classify_release("Epic", Some(12), None, None, false),
            "album"
        );
        // type_is matches either the item's or the group's type.
        assert_eq!(
            classify_release("X", Some(12), None, Some("single"), false),
            "single"
        );
    }

    /// The arms `classify_precedence` does not reach. Each one changes the TYPE
    /// label a row shows and, through the group's `is_compilation`, whether the
    /// row is pre-checked.
    #[test]
    fn classify_release_remaining_arms() {
        // The compilation arm is `.contains("compilation")` on EITHER type
        // string — not an equality test.
        assert_eq!(
            classify_release("X", Some(12), Some("Compilations"), None, false),
            "compilation"
        );
        assert_eq!(
            classify_release("X", Some(12), None, Some("epCompilation"), false),
            "compilation"
        );
        // live: the other two words, and punctuation counts as a boundary.
        assert_eq!(
            classify_release("The Concert", Some(20), None, None, false),
            "live"
        );
        assert_eq!(
            classify_release("Live!", Some(20), None, None, false),
            "live"
        );
        // "alive" must NOT match the `live` word test.
        assert_eq!(
            classify_release("Alive", Some(20), None, None, false),
            "album"
        );
        // The GROUP's type beats the item's for live (type_is is an OR).
        assert_eq!(
            classify_release("X", Some(20), Some("album"), Some("live"), false),
            "live"
        );
        // type_is("album") short-circuits BEFORE the track-count heuristic, so a
        // 2-track release typed "album" stays an album (not a single).
        assert_eq!(
            classify_release("X", Some(2), Some("album"), None, false),
            "album"
        );
        // …and type_is("single") wins over a 20-track count.
        assert_eq!(
            classify_release("X", Some(20), Some("single"), None, false),
            "single"
        );
        // Unknown track count falls through to album.
        assert_eq!(classify_release("X", None, None, None, false), "album");
        // The heuristic boundaries: 3 -> single, 4 -> ep, 6 -> ep, 7 -> album.
        assert_eq!(classify_release("X", Some(3), None, None, false), "single");
        assert_eq!(classify_release("X", Some(4), None, None, false), "ep");
        assert_eq!(classify_release("X", Some(6), None, None, false), "ep");
        assert_eq!(classify_release("X", Some(7), None, None, false), "album");
    }

    #[test]
    fn quality_score_ordering() {
        // bit depth dominates, then rate, then format bonus.
        assert!(
            quality_score(Some(24), Some(96.0), "FLAC")
                > quality_score(Some(16), Some(192.0), "FLAC")
        );
        assert!(
            quality_score(Some(16), Some(44.1), "FLAC")
                > quality_score(Some(16), Some(44.1), "MP3")
        );
        // Unknown format sits between mp3 (0) and flac (1000).
        let unknown = quality_score(Some(16), Some(44.1), "OGG");
        assert!(unknown > quality_score(Some(16), Some(44.1), "MP3"));
        assert!(unknown < quality_score(Some(16), Some(44.1), "FLAC"));
    }

    /// The exact arithmetic, not just the ordering — this score picks which
    /// candidate takes the PRIMARY slot of a group, so the constants and the
    /// `None` defaults (16-bit / 44.1 kHz) are load-bearing.
    #[test]
    fn quality_score_exact_values() {
        assert_eq!(quality_score(Some(24), Some(96.0), "FLAC"), 240_097_000);
        assert_eq!(quality_score(Some(16), Some(44.1), "MP3"), 160_044_100);
        assert_eq!(quality_score(Some(16), Some(44.1), "AAC"), 160_044_100);
        assert_eq!(quality_score(Some(16), Some(44.1), "ALAC"), 160_045_100);
        // Missing depth AND rate default to 16-bit / 44.1 kHz; an unrecognised
        // container gets the middle 500 bonus.
        assert_eq!(quality_score(None, None, ""), 160_044_600);
        // A rate already in Hz is NOT normalised here (the reference multiplies
        // blindly), so an accidental Hz value scores absurdly high — pinned so a
        // "fix" in the fetchers is not mistaken for a change here.
        assert_eq!(quality_score(Some(16), Some(44100.0), "FLAC"), 204_101_000);
        // AIFF is lossless but not in the bonus list, so it scores BELOW FLAC.
        assert!(
            quality_score(Some(24), Some(96.0), "AIFF")
                < quality_score(Some(24), Some(96.0), "FLAC")
        );
    }

    #[test]
    fn build_groups_dedupes_and_prefers_qobuz_on_ties() {
        let groups = build_groups(vec![
            cand("local", "l1", "Album One", Some(2001), 100),
            cand("qobuz", "q1", "Album One (Deluxe)", Some(2001), 100),
            // exact duplicate key -> dropped
            cand("local", "l1", "Album One", Some(2001), 999),
            cand("local", "l2", "Album Two", Some(2002), 50),
        ]);
        assert_eq!(groups.len(), 2);
        // Bucket order is first-seen: "album one|2001" then "album two|2002".
        assert_eq!(groups[0].key, "album one|2001");
        // Equal quality -> qobuz takes the primary slot.
        assert_eq!(groups[0].primary.source, "qobuz");
        assert_eq!(groups[0].alternates.len(), 1);
        assert_eq!(groups[0].alternates[0].source, "local");
        assert!(groups[1].alternates.is_empty());
    }

    #[test]
    fn ordered_groups_release_date_puts_nulls_last() {
        let groups = build_groups(vec![
            cand("qobuz", "a", "Bee", None, 10),
            cand("qobuz", "b", "Cee", Some(1999), 10),
            cand("qobuz", "c", "Aye", Some(1990), 10),
        ]);
        let by_date = ordered_groups(&groups, "release_date");
        assert_eq!(
            by_date.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
            vec!["Aye", "Cee", "Bee"]
        );
        let by_title = ordered_groups(&groups, "title");
        assert_eq!(
            by_title
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Aye", "Bee", "Cee"]
        );
        let manual = ordered_groups(&groups, "manual");
        assert_eq!(
            manual.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
            vec!["Bee", "Cee", "Aye"]
        );
    }

    /// `release_date`'s tie-break is NOT the title, despite the reference's own
    /// comment: equal `Some(year)` compares Equal, and `sort_by` is stable, so
    /// same-year groups keep INSERTION order. Only the null bucket falls through
    /// to the lowercase-title compare (the `(None, None)` arm).
    #[test]
    fn ordered_groups_release_date_ties_keep_insertion_order() {
        let same_year = build_groups(vec![
            cand("qobuz", "a", "Zebra", Some(1990), 10),
            cand("qobuz", "b", "Apple", Some(1990), 10),
        ]);
        assert_eq!(
            ordered_groups(&same_year, "release_date")
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Zebra", "Apple"]
        );
        let no_year = build_groups(vec![
            cand("qobuz", "a", "Zebra", None, 10),
            cand("qobuz", "b", "Apple", None, 10),
        ]);
        assert_eq!(
            ordered_groups(&no_year, "release_date")
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Apple", "Zebra"]
        );
        // An unknown order-by string falls into the release_date arm, not manual.
        assert_eq!(
            ordered_groups(&no_year, "whatever")
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Apple", "Zebra"]
        );
    }

    /// Three non-compilation groups + one compilation, built the way `install`
    /// seeds them.
    fn selection_fixture() -> (Vec<Group>, usize) {
        let groups = build_groups(vec![
            cand("qobuz", "a", "Album One", Some(2001), 10),
            cand("qobuz", "b", "Greatest Hits", Some(2002), 10),
            cand("qobuz", "c", "Album Two", Some(2003), 10),
        ]);
        assert_eq!(groups.len(), 3);
        // `cand` derives is_compilation from the title, so group 1 IS one.
        assert!(!groups[0].is_compilation);
        assert!(groups[1].is_compilation);
        assert!(!groups[2].is_compilation);
        (groups, 1)
    }

    fn seed_default_selection(s: &mut Session) {
        s.checked.clear();
        let snapshot: Vec<(String, bool, String)> = s
            .groups
            .iter()
            .map(|g| (g.key.clone(), g.is_compilation, candidate_key(&g.primary)))
            .collect();
        for (key, is_comp, primary) in snapshot {
            let mut set = Vec::new();
            if !is_comp {
                set.push(primary);
            }
            s.checked.insert(key, set);
        }
    }

    /// The header tri-state and the footer count are computed over DIFFERENT
    /// sets: the tri-state looks at non-compilation PRIMARIES only, the count
    /// sums every checked candidate. A compilation-only selection therefore
    /// leaves the header box EMPTY while Create is enabled with count 1.
    #[test]
    fn selection_maths_ignore_compilations() {
        let (groups, comp_idx) = selection_fixture();
        let comp_key = groups[comp_idx].key.clone();
        let comp_primary = candidate_key(&groups[comp_idx].primary);
        let a_key = groups[0].key.clone();
        let a_primary = candidate_key(&groups[0].primary);
        let c_key = groups[2].key.clone();

        let mut s = Session {
            groups,
            ..Session::default()
        };

        // An empty session is neither all- nor some-checked.
        let empty = Session::default();
        assert!(!all_primaries_checked(&empty));
        assert!(!some_primaries_checked(&empty));
        assert_eq!(selected_count(&empty), 0);

        // Default seed: the compilation is unchecked yet `all` is TRUE (the
        // `g.is_compilation ||` arm treats it as satisfied).
        seed_default_selection(&mut s);
        assert!(all_primaries_checked(&s));
        assert!(!some_primaries_checked(&s));
        assert_eq!(selected_count(&s), 2);

        // Uncheck one non-compilation primary -> partial.
        s.checked.get_mut(&a_key).unwrap().clear();
        assert!(!all_primaries_checked(&s));
        assert!(some_primaries_checked(&s));
        assert_eq!(selected_count(&s), 1);

        // Compilation-only selection: the header box is EMPTY (neither all nor
        // some) but the footer still counts 1 and Create is enabled.
        s.checked
            .get_mut(&comp_key)
            .unwrap()
            .push(comp_primary.clone());
        s.checked.get_mut(&c_key).unwrap().clear();
        assert!(!all_primaries_checked(&s));
        assert!(!some_primaries_checked(&s));
        assert_eq!(selected_count(&s), 1);

        // Alternates count toward the footer but never toward the tri-state.
        s.checked
            .get_mut(&a_key)
            .unwrap()
            .push("qobuz|not-a-primary".to_string());
        assert_eq!(selected_count(&s), 2);
        assert!(!all_primaries_checked(&s));
        assert!(!some_primaries_checked(&s));
        assert!(!is_checked(&s, &a_key, &a_primary));
    }

    /// `toggle_all` must touch non-compilation PRIMARIES only: a hand-checked
    /// compilation and any checked alternate survive both directions.
    #[test]
    fn toggle_all_touches_non_compilation_primaries_only() {
        let (groups, comp_idx) = selection_fixture();
        let comp_key = groups[comp_idx].key.clone();
        let comp_primary = candidate_key(&groups[comp_idx].primary);
        let a_key = groups[0].key.clone();
        let a_primary = candidate_key(&groups[0].primary);

        session(|s| {
            *s = Session {
                groups: groups.clone(),
                ..Session::default()
            };
            seed_default_selection(s);
            // The user also ticked the compilation and one alternate by hand.
            s.checked
                .get_mut(&comp_key)
                .unwrap()
                .push(comp_primary.clone());
            s.checked
                .get_mut(&a_key)
                .unwrap()
                .push("qobuz|alt".to_string());
        });

        // All non-compilation primaries were checked -> this UNCHECKS them.
        toggle_all();
        session(|s| {
            assert!(!is_checked(s, &a_key, &a_primary));
            assert!(
                is_checked(s, &comp_key, &comp_primary),
                "compilation untouched"
            );
            assert!(is_checked(s, &a_key, "qobuz|alt"), "alternate untouched");
            assert_eq!(selected_count(s), 2);
        });

        // …and back.
        toggle_all();
        session(|s| {
            assert!(all_primaries_checked(s));
            assert!(is_checked(s, &comp_key, &comp_primary));
            assert!(is_checked(s, &a_key, "qobuz|alt"));
            assert_eq!(selected_count(s), 4);
        });

        // Leave the process-global session clean for any other test.
        session(|s| *s = Session::default());
    }

    #[test]
    fn badge_tiers_match_qualitybadgefull_vocabulary() {
        assert_eq!(badge("FLAC", Some(24), Some(96.0)).0, "hires");
        assert_eq!(badge("FLAC", Some(16), Some(44.1)).0, "cd");
        assert_eq!(badge("MP3", Some(16), Some(44.1)).0, "mp3");
        assert_eq!(
            badge("FLAC", None, None),
            ("lossless".to_string(), "FLAC".to_string())
        );
        assert_eq!(badge("Unknown", None, None).0, "");
        assert_eq!(
            badge("DSF", Some(1), Some(2822.4)),
            ("dsd".to_string(), "DSD64".to_string())
        );
    }
}
