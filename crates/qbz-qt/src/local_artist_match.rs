//! Artist IDENTITY for the Local Library: name normalization, credit
//! splitting, "does this album credit that artist", and the spelling merge
//! behind the Artists tab.
//!
//! Ported 1:1 from the shipping Slint controller
//! (`crates/qbz/src/local_library.rs`): `fold_diacritic` (:3170),
//! `normalize_artist` (:3188), `split_credit` (:3206),
//! `build_artist_album_ids` (:3218), `merge_artists` (:3261) and
//! `album_matches_artist` (:3368). Everything here is a PURE function — no
//! Qt, no DB, no globals — which is why the whole module is unit-tested at
//! the bottom instead of only being exercised through the UI.
//!
//! # Why this is not `qbz_mixtape::shuffle::normalize_artist`
//!
//! That one exists to answer a DIFFERENT question (are these two queue rows
//! the same song by the same act) and deliberately PRESERVES punctuation and
//! parentheses — its doc comment says so: "`Foo (band)` must not collapse to
//! `Foo`". Artist identity here needs the opposite: the Slint rule collapses
//! every run of non-alphanumerics to a single space so `Sigur Rós`,
//! `Sigur Ros` and `sigur  ros` are one artist, and so `Beyoncé` merges with
//! `Beyonce`. The two diacritic tables differ as well (mixtape's strips a
//! wider Unicode range, this one is the hand-written Latin table the Slint
//! ships). Reusing it would silently change which artists merge, so this is a
//! second function on purpose, not an accidental third copy.
//!
//! `local_rows::artist_key` is NOT a normalizer either — it is the artwork
//! cache key (`artist:{name}`) and must stay keyed on the DISPLAY name, since
//! that is what the rows carry.

use std::collections::{HashMap, HashSet};

const ARTIST_FAMILY_SEPARATOR: &str = " • ";

#[derive(Default)]
struct ArtistFamilyEvidence {
    display_counts: HashMap<String, usize>,
    suffixes: HashSet<String>,
    members: HashSet<String>,
}

/// Corpus-derived aliases for repeated `collection • contributor` credits.
/// The full source names remain outside this value; it only answers which
/// repeated members share one root identity.
#[derive(Default)]
pub struct ArtistFamilyAliases {
    by_member: HashMap<String, String>,
}

impl ArtistFamilyAliases {
    fn canonical_display<'a>(&'a self, name: &str) -> Option<&'a str> {
        self.by_member
            .get(&normalize_artist(name))
            .map(String::as_str)
    }

    fn canonical_key(&self, name: &str) -> String {
        self.canonical_display(name)
            .map(normalize_artist)
            .unwrap_or_else(|| normalize_artist(name))
    }
}

/// Canonical display names credited by one album. This is the presentation
/// companion to [`album_matches_artist_with_aliases`]: fold repeated
/// `collection • contributor` families to their corpus-backed root and
/// deduplicate by normalized identity while preserving first display
/// spelling. Deliberately do not split `AC/DC` or `Simon & Garfunkel`: the
/// artist rail treats those full display names as acts, and this facet must
/// expose the same identities rather than manufacture phantom contributors.
pub fn album_credit_names(
    artist: &str,
    all_artists: &str,
    aliases: &ArtistFamilyAliases,
) -> Vec<String> {
    let mut out = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();
    let raw = all_artists.split(',').chain(std::iter::once(artist));
    for value in raw {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let display = aliases.canonical_display(value).unwrap_or(value);
        let key = normalize_artist(display);
        if !key.is_empty() && seen.insert(key) {
            out.push(display.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Fold a common Latin accented char to its ASCII base (best-effort, no
/// `unicode-normalization` dep). Covers Spanish/European music metadata; the
/// uncovered tail just won't merge across diacritics.
/// (`local_library.rs:3170`, table copied verbatim.)
fn fold_diacritic(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ė' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ō' | 'ø' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        'ý' | 'ÿ' => 'y',
        'ß' => 's',
        _ => c,
    }
}

/// Normalize an artist name for merge/match: lowercase, fold diacritics,
/// collapse every run of non-alphanumerics to a single space, trim. So
/// "Alice In Chains" and "alice  in chains" both -> "alice in chains".
pub fn normalize_artist(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_space = false;
    for ch in name.to_lowercase().chars() {
        let c = fold_diacritic(ch);
        // Artist identity is global metadata, not an ASCII search key. Using
        // `is_ascii_alphanumeric` erased Japanese/CJK names entirely; the
        // synthetic `family • latin contributor` rows only survived because
        // their suffix happened to contain ASCII.
        if c.is_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Split a credit string into individual artist names on the usual
/// separators (comma already handled by the caller for `all_artists`).
pub fn split_credit(s: &str) -> Vec<String> {
    s.split([',', '&', '/', ';'])
        .flat_map(|p| {
            p.split(" feat ")
                .flat_map(|q| q.split(" ft "))
                .flat_map(|q| q.split(" featuring "))
                .flat_map(|q| q.split(" with "))
                .map(|q| q.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn split_artist_family(name: &str) -> Option<(&str, &str)> {
    let (prefix, suffix) = name.split_once(ARTIST_FAMILY_SEPARATOR)?;
    let prefix = prefix.trim();
    let suffix = suffix.trim();
    (!prefix.is_empty() && !suffix.is_empty()).then_some((prefix, suffix))
}

/// A single bullet-bearing name may be intentional. A shared prefix with two
/// distinct suffixes is the evidence that this collection encoded an album
/// family as many synthetic root artists.
pub fn build_artist_family_aliases(names: &[&str]) -> ArtistFamilyAliases {
    let mut families = HashMap::<String, ArtistFamilyEvidence>::new();
    for name in names {
        let Some((prefix, suffix)) = split_artist_family(name) else {
            continue;
        };
        let family_key = normalize_artist(prefix);
        let suffix_key = normalize_artist(suffix);
        let member_key = normalize_artist(name);
        if family_key.is_empty() || suffix_key.is_empty() || member_key.is_empty() {
            continue;
        }
        let evidence = families.entry(family_key).or_default();
        evidence.suffixes.insert(suffix_key);
        evidence.members.insert(member_key);
        *evidence
            .display_counts
            .entry(prefix.to_string())
            .or_default() += 1;
    }

    let mut aliases = ArtistFamilyAliases::default();
    for evidence in families.into_values() {
        if evidence.suffixes.len() < 2 {
            continue;
        }
        let mut displays = evidence.display_counts.into_iter().collect::<Vec<_>>();
        displays.sort_by(|(left_name, left_count), (right_name, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| left_name.cmp(right_name))
        });
        let Some((canonical, _)) = displays.into_iter().next() else {
            continue;
        };
        for member in evidence.members {
            aliases.by_member.insert(member, canonical.clone());
        }
    }
    aliases
}

// ---------------------------------------------------------------------------
// Album <-> artist matching
// ---------------------------------------------------------------------------

/// The album facts the two predicates below need. A borrowed view rather than
/// `qbz_library::LocalAlbum` so the caller can feed EITHER the raw query rows
/// (the artists loader) or the cached transport rows the Artists tab renders
/// (`local_rows::AlbumRow`) without a conversion pass.
pub struct AlbumCredit<'a> {
    pub id: &'a str,
    pub artist: &'a str,
    /// Comma-separated contributor list the DB aggregates per album ("" when
    /// the query did not populate it).
    pub all_artists: &'a str,
    /// On-disk cover / Plex thumb path ("" when the album has none).
    pub artwork_path: &'a str,
    /// The album's own source word, so a portrait borrowed from its cover
    /// keeps that cover's provenance. Without it the artwork index would have
    /// to guess what `artwork_path` is from its characters, which is exactly
    /// the sniffing design 02 §9 stage 4 removes.
    pub source: &'a str,
}

/// Does this album credit the artist whose normalized name is `nsel` — as
/// primary, inside `all_artists`, or as one part of a multi-artist credit?
///
/// Exact comparison of normalized parts, never a substring test: "Air" must
/// not match "Airbourne" or "Blair", and an album credited "A & B" must
/// appear under "B". Repeated family aliases are resolved before comparison.
pub fn album_matches_artist_with_aliases(
    artist: &str,
    all_artists: &str,
    nsel: &str,
    aliases: &ArtistFamilyAliases,
) -> bool {
    if nsel == "various artists" {
        return normalize_artist(artist) == "various artists";
    }
    if aliases.canonical_key(artist) == nsel {
        return true;
    }
    for part in all_artists.split(',') {
        if aliases.canonical_key(part) == nsel {
            return true;
        }
    }
    for part in split_credit(artist) {
        if aliases.canonical_key(&part) == nsel {
            return true;
        }
    }
    false
}

/// Build the per-normalized-artist set of album ids, so merged rows get an
/// accurate unique album count independent of per-track spelling.
fn build_artist_album_ids_with_aliases(
    albums: &[AlbumCredit<'_>],
    aliases: &ArtistFamilyAliases,
) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for al in albums {
        if !al.all_artists.is_empty() {
            for part in al.all_artists.split(',') {
                let n = aliases.canonical_key(part);
                if n.is_empty() || n == "various artists" {
                    continue;
                }
                map.entry(n).or_default().insert(al.id.to_string());
            }
        } else {
            let n = aliases.canonical_key(al.artist);
            if !n.is_empty() && n != "various artists" {
                map.entry(n).or_default().insert(al.id.to_string());
            }
        }
    }
    map
}

// ---------------------------------------------------------------------------
// The spelling merge
// ---------------------------------------------------------------------------

/// One artist as it goes IN to the merge (a `qbz_library::LocalArtist` row, or
/// an aggregated Plex artist).
pub struct ArtistInput {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    /// "local" | "plex" | "jellyfin" | "subsonic".
    pub source: &'static str,
}

/// One artist as it comes OUT: a single canonical row per normalized name.
pub struct MergedArtist {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    /// Portrait SOURCE (custom/cached url or path, Plex thumb, album cover);
    /// "" when nothing is known and the row falls back to the placeholder.
    pub image_path: String,
    /// WHICH source [`MergedArtist::image_path`] came from — never "mixed".
    ///
    /// Distinct from [`MergedArtist::source`] on purpose: that one describes
    /// the artist's LIBRARY provenance and is legitimately "mixed" when a name
    /// has both local and Plex rows. A portrait, however, comes from exactly
    /// ONE of the three chain tiers, and the artwork index needs to know which
    /// — "mixed" is not a source a token can be interpreted by.
    pub image_source: String,
    /// One source word, or "mixed" when more than one source contributes.
    pub source: String,
}

/// Collapse normalized-equal artist spellings into ONE canonical row and
/// attach accurate album counts + a portrait path.
///
/// Canonical = the variant with the most albums (tie: most tracks); merged
/// track count = the sum across variants; album count = the size of the
/// album-id set for that normalized name (which cross-lists `all_artists`),
/// falling back to the DB's own per-variant count when the album set is empty
/// (the artists tab loaded before any album did).
///
/// Portrait chain: custom/cached image -> representative Plex thumb ->
/// representative album cover. The last arm is GATED on
/// `album_thumb_fallback` (set only when Plex is ON), 1:1 with the reference:
/// with Plex off a local artist with no custom portrait keeps `image_path`
/// empty, which is what leaves the Qobuz portrait fetch its slot.
pub fn merge_artists(
    mut artists: Vec<ArtistInput>,
    albums: &[AlbumCredit<'_>],
    custom_images: &HashMap<String, String>,
    plex_portraits: &HashMap<String, String>,
    album_thumb_fallback: bool,
) -> Vec<MergedArtist> {
    let mut family_names = artists
        .iter()
        .map(|artist| artist.name.as_str())
        .chain(albums.iter().map(|album| album.artist))
        .collect::<Vec<_>>();
    for album in albums {
        family_names.extend(album.all_artists.split(',').filter(|name| !name.is_empty()));
    }
    let family_aliases = build_artist_family_aliases(&family_names);
    let album_ids = build_artist_album_ids_with_aliases(albums, &family_aliases);
    for artist in &mut artists {
        if let Some(canonical) = family_aliases.canonical_display(&artist.name) {
            artist.name = canonical.to_string();
        }
    }
    let norm_imgs: HashMap<String, String> = custom_images
        .iter()
        .map(|(k, v)| (normalize_artist(k), v.clone()))
        .collect();
    // (path, the album's OWN source word) — the tier is Plex-gated but the
    // album it borrows from can still be a local one, so the word travels.
    let mut album_thumbs: HashMap<String, (String, String)> = HashMap::new();
    if album_thumb_fallback {
        for al in albums {
            if !al.artwork_path.is_empty() {
                album_thumbs
                    .entry(family_aliases.canonical_key(al.artist))
                    .or_insert_with(|| (al.artwork_path.to_string(), al.source.to_string()));
            }
        }
    }

    let mut groups: HashMap<String, Vec<ArtistInput>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for a in artists {
        let n = normalize_artist(&a.name);
        if n.is_empty() {
            continue;
        }
        if !groups.contains_key(&n) {
            order.push(n.clone());
        }
        groups.entry(n).or_default().push(a);
    }

    let mut out: Vec<MergedArtist> = Vec::with_capacity(order.len());
    for n in order {
        let Some(variants) = groups.remove(&n) else {
            continue;
        };
        let album_set_len = album_ids.get(&n).map(|s| s.len()).unwrap_or(0) as u32;
        let mut sources: Vec<&str> = Vec::new();
        for variant in &variants {
            if !sources.contains(&variant.source) {
                sources.push(variant.source);
            }
        }
        let source = if sources.len() == 1 {
            sources[0]
        } else {
            "mixed"
        };
        // If no album-credit document is available (notably a server-only
        // profile without library.db), counts from distinct sources are
        // disjoint because their album ids are namespaced. Within one source,
        // spelling variants can overlap, so retain only that source's maximum.
        let mut fallback_albums_by_source: HashMap<&str, u32> = HashMap::new();
        for variant in &variants {
            fallback_albums_by_source
                .entry(variant.source)
                .and_modify(|count| *count = (*count).max(variant.album_count))
                .or_insert(variant.album_count);
        }
        let fallback_album_count = fallback_albums_by_source.values().sum();
        let (canonical, album_count, track_count) = if variants.len() == 1 {
            let v = &variants[0];
            let ac = if album_set_len > 0 {
                album_set_len
            } else {
                v.album_count
            };
            (v.name.clone(), ac, v.track_count)
        } else {
            // `max_by` keeps the LAST maximum, like the reference.
            let canon = variants
                .iter()
                .max_by(|a, b| {
                    a.album_count
                        .cmp(&b.album_count)
                        .then(a.track_count.cmp(&b.track_count))
                })
                .expect("non-empty group");
            let total_tracks: u32 = variants.iter().map(|v| v.track_count).sum();
            let ac = if album_set_len > 0 {
                album_set_len
            } else {
                fallback_album_count
            };
            (canon.name.clone(), ac, total_tracks)
        };
        // The three tiers, each carrying WHERE it came from. Order is the
        // reference's: custom/cached image -> Plex portrait -> album cover.
        let (image_path, image_source) = if let Some(p) = norm_imgs.get(&n) {
            // Custom portraits are written by this app: a local path or a
            // cached url, never a server-relative thumb.
            (p.clone(), "local")
        } else if let Some(p) = plex_portraits.get(&n) {
            (p.clone(), "plex")
        } else if let Some((p, src)) = album_thumbs.get(&n) {
            (p.clone(), src.as_str())
        } else {
            (String::new(), "local")
        };
        out.push(MergedArtist {
            name: canonical,
            album_count,
            track_count,
            image_path,
            image_source: image_source.to_string(),
            source: source.to_string(),
        });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn album_matches_artist(artist: &str, all_artists: &str, nsel: &str) -> bool {
        album_matches_artist_with_aliases(
            artist,
            all_artists,
            nsel,
            &ArtistFamilyAliases::default(),
        )
    }

    fn credit<'a>(id: &'a str, artist: &'a str, all: &'a str, art: &'a str) -> AlbumCredit<'a> {
        credit_from("local", id, artist, all, art)
    }

    fn credit_from<'a>(
        source: &'a str,
        id: &'a str,
        artist: &'a str,
        all: &'a str,
        art: &'a str,
    ) -> AlbumCredit<'a> {
        AlbumCredit {
            source,
            id,
            artist,
            all_artists: all,
            artwork_path: art,
        }
    }

    fn input(name: &str, albums: u32, tracks: u32, source: &'static str) -> ArtistInput {
        ArtistInput {
            name: name.to_string(),
            album_count: albums,
            track_count: tracks,
            source,
        }
    }

    // --- normalize_artist --------------------------------------------------

    #[test]
    fn normalize_lowercases_and_collapses_whitespace() {
        assert_eq!(normalize_artist("Alice In Chains"), "alice in chains");
        assert_eq!(normalize_artist("alice  in   chains"), "alice in chains");
        assert_eq!(normalize_artist("  Air  "), "air");
    }

    /// PARITY-DEBT #7: the merge key used to be `trim().to_lowercase()`, so
    /// these were two artists with split album/track counts.
    #[test]
    fn normalize_folds_diacritics() {
        assert_eq!(normalize_artist("Beyoncé"), normalize_artist("Beyonce"));
        assert_eq!(normalize_artist("Sigur Rós"), "sigur ros");
        assert_eq!(normalize_artist("Mötley Crüe"), "motley crue");
        assert_eq!(normalize_artist("Françoise"), "francoise");
    }

    #[test]
    fn normalize_preserves_non_ascii_scripts() {
        assert_eq!(
            normalize_artist("新世紀エヴァンゲリオン"),
            "新世紀エヴァンゲリオン"
        );
        assert_eq!(normalize_artist("林原めぐみ"), "林原めぐみ");
    }

    #[test]
    fn normalize_collapses_punctuation_to_one_space() {
        assert_eq!(
            normalize_artist("Godspeed You! Black Emperor"),
            "godspeed you black emperor"
        );
        assert_eq!(normalize_artist("Sunn O)))"), "sunn o");
        // A run of separators collapses to ONE space, and the ends are trimmed.
        assert_eq!(
            normalize_artist("...And You Will Know Us..."),
            "and you will know us"
        );
        // This is exactly what makes it different from the mixtape normalizer,
        // which keeps the parens.
        assert_eq!(normalize_artist("Foo (band)"), "foo band");
    }

    #[test]
    fn normalize_empty_and_punctuation_only() {
        assert_eq!(normalize_artist(""), "");
        assert_eq!(normalize_artist("   "), "");
        assert_eq!(normalize_artist("-- / --"), "");
    }

    // --- split_credit ------------------------------------------------------

    #[test]
    fn split_credit_splits_on_every_separator() {
        assert_eq!(split_credit("A & B"), vec!["A ", " B"]);
        assert_eq!(split_credit("A; B/C"), vec!["A", " B", "C"]);
        assert_eq!(
            split_credit("Massive Attack feat Tracey Thorn"),
            vec!["Massive Attack", "Tracey Thorn"]
        );
        assert_eq!(split_credit("A ft B"), vec!["A", "B"]);
        assert_eq!(split_credit("A featuring B"), vec!["A", "B"]);
        assert_eq!(split_credit("A with B"), vec!["A", "B"]);
    }

    #[test]
    fn split_credit_leaves_a_plain_name_alone() {
        assert_eq!(split_credit("Radiohead"), vec!["Radiohead"]);
    }

    // --- album_matches_artist ---------------------------------------------

    /// PARITY-DEBT #8, the false positives: the QML matched with a SUBSTRING
    /// `indexOf` over `allArtists`, so selecting "Air" listed everything whose
    /// credit merely CONTAINED "air".
    #[test]
    fn no_substring_false_positives() {
        let nsel = normalize_artist("Air");
        assert!(!album_matches_artist("Airbourne", "Airbourne", &nsel));
        assert!(!album_matches_artist("Blair", "Blair", &nsel));
        assert!(!album_matches_artist("Air Supply", "Air Supply", &nsel));
        assert!(album_matches_artist("Air", "Air", &nsel));
    }

    /// PARITY-DEBT #8, the misses: an album credited "A & B" never appeared
    /// under "B" because the old rule only did an exact compare on `artist`.
    #[test]
    fn split_credits_are_matched_part_by_part() {
        let b = normalize_artist("Burial");
        assert!(album_matches_artist("Four Tet & Burial", "", &b));
        assert!(album_matches_artist("Four Tet feat Burial", "", &b));
        assert!(album_matches_artist("Four Tet/Burial", "", &b));
        assert!(album_matches_artist("Four Tet; Burial", "", &b));
        assert!(album_matches_artist("Four Tet with Burial", "", &b));
    }

    #[test]
    fn all_artists_parts_are_matched_exactly() {
        let n = normalize_artist("Portishead");
        assert!(album_matches_artist(
            "Various",
            "Massive Attack,Portishead,Tricky",
            &n
        ));
        assert!(!album_matches_artist(
            "Various",
            "Massive Attack,Tricky",
            &n
        ));
        // Diacritic + spacing differences still merge.
        assert!(album_matches_artist(
            "x",
            "Sigur  Ros",
            &normalize_artist("Sigur Rós")
        ));
    }

    #[test]
    fn various_artists_is_its_own_bucket() {
        let va = normalize_artist("Various Artists");
        assert!(album_matches_artist("Various Artists", "A,B,C", &va));
        // A compilation whose primary credit is a real artist is NOT "Various
        // Artists", even though its contributor list is long.
        assert!(!album_matches_artist(
            "Nina Simone",
            "Various Artists,Nina Simone",
            &va
        ));
    }

    // --- merge_artists -----------------------------------------------------

    /// PARITY-DEBT #7: "Beyoncé" and "Beyonce" are ONE artist, with summed
    /// track counts and an album count taken from the album set.
    #[test]
    fn spelling_variants_merge_into_one_row() {
        let albums = vec![
            credit("a1", "Beyoncé", "Beyoncé", ""),
            credit("a2", "Beyonce", "Beyonce", ""),
        ];
        let merged = merge_artists(
            vec![
                input("Beyoncé", 1, 12, "local"),
                input("Beyonce", 1, 4, "local"),
            ],
            &albums,
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].track_count, 16);
        // Two DISTINCT album ids under one normalized name.
        assert_eq!(merged[0].album_count, 2);
        assert_eq!(merged[0].source, "local");
    }

    #[test]
    fn canonical_spelling_is_the_one_with_most_albums() {
        let merged = merge_artists(
            vec![
                input("beyonce", 1, 4, "local"),
                input("Beyoncé", 3, 12, "local"),
            ],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "Beyoncé");
        // No album set -> the canonical variant's own DB count is kept.
        assert_eq!(merged[0].album_count, 3);
    }

    #[test]
    fn local_and_plex_same_artist_merge_as_mixed() {
        let merged = merge_artists(
            vec![
                input("Radiohead", 2, 20, "local"),
                input("radiohead", 1, 10, "plex"),
            ],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, "mixed");
        assert_eq!(merged[0].track_count, 30);
    }

    #[test]
    fn exclusively_remote_artist_keeps_its_counts_and_source() {
        let merged = merge_artists(
            vec![input("Remote Only", 7, 83, "jellyfin")],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "Remote Only");
        assert_eq!(merged[0].album_count, 7);
        assert_eq!(merged[0].track_count, 83);
        assert_eq!(merged[0].source, "jellyfin");
    }

    #[test]
    fn remote_only_artist_counts_add_across_distinct_servers() {
        let merged = merge_artists(
            vec![
                input("Remote Only", 7, 83, "jellyfin"),
                input("remote only", 4, 19, "subsonic"),
            ],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].album_count, 11);
        assert_eq!(merged[0].track_count, 102);
        assert_eq!(merged[0].source, "mixed");
    }

    #[test]
    fn repeated_bullet_families_collapse_but_one_off_collaborations_survive() {
        let family = "新世紀エヴァンゲリオン";
        let shiro = format!("{family} • Shiro Sagisu");
        let megumi = format!("{family} • Megumi Hayashibara");
        let yoko = format!("{family} • Yoko Takahashi");
        let albums = vec![
            credit("eva-0", family, "", ""),
            credit("eva-1", &shiro, "", ""),
            credit("eva-2", &megumi, "", ""),
            credit("eva-3", &yoko, "", ""),
            credit("duet-0", "Alice • Bob", "", ""),
        ];
        let merged = merge_artists(
            vec![
                input(family, 1, 40, "subsonic"),
                input(&shiro, 1, 10, "subsonic"),
                input(&megumi, 1, 20, "subsonic"),
                input(&yoko, 1, 30, "subsonic"),
                input("鷺巣詩郎", 1, 10, "subsonic"),
                input("林原めぐみ", 1, 20, "subsonic"),
                input("高橋洋子", 1, 30, "subsonic"),
                input("Alice • Bob", 1, 7, "subsonic"),
            ],
            &albums,
            &HashMap::new(),
            &HashMap::new(),
            false,
        );

        let family_rows = merged
            .iter()
            .filter(|artist| artist.name == family)
            .collect::<Vec<_>>();
        assert_eq!(family_rows.len(), 1);
        assert_eq!(family_rows[0].album_count, 4);
        assert_eq!(family_rows[0].track_count, 100);
        assert!(merged.iter().any(|artist| artist.name == "鷺巣詩郎"));
        assert!(merged.iter().any(|artist| artist.name == "林原めぐみ"));
        assert!(merged.iter().any(|artist| artist.name == "高橋洋子"));
        assert!(merged.iter().any(|artist| artist.name == "Alice • Bob"));

        let names = albums.iter().map(|album| album.artist).collect::<Vec<_>>();
        let aliases = build_artist_family_aliases(&names);
        assert!(album_matches_artist_with_aliases(
            &shiro,
            "",
            &normalize_artist(family),
            &aliases,
        ));
        assert_eq!(album_credit_names(&shiro, &megumi, &aliases), [family]);
        assert_eq!(
            album_credit_names("AC/DC", "Simon & Garfunkel", &aliases),
            ["Simon & Garfunkel", "AC/DC"]
        );
        assert!(!album_matches_artist_with_aliases(
            "Alice • Bob",
            "",
            &normalize_artist("Alice"),
            &aliases,
        ));
    }

    #[test]
    fn album_count_cross_lists_all_artists() {
        // One album, credited to two artists: BOTH get a count of 1.
        let albums = vec![credit("a1", "Four Tet", "Four Tet,Burial", "")];
        let merged = merge_artists(
            vec![
                input("Four Tet", 1, 3, "local"),
                input("Burial", 0, 3, "local"),
            ],
            &albums,
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        let burial = merged
            .iter()
            .find(|m| m.name == "Burial")
            .expect("burial row");
        assert_eq!(burial.album_count, 1);
    }

    #[test]
    fn portrait_chain_prefers_custom_then_plex_then_cover() {
        let albums = vec![credit("a1", "Air", "Air", "/covers/air.jpg")];
        let mut custom = HashMap::new();
        custom.insert("AIR".to_string(), "/custom/air.png".to_string());
        let mut plex = HashMap::new();
        plex.insert("air".to_string(), "/library/metadata/1/thumb".to_string());

        // custom wins (and its KEY is normalized too, so "AIR" matches "air")
        let m = merge_artists(
            vec![input("Air", 1, 9, "local")],
            &albums,
            &custom,
            &plex,
            true,
        );
        assert_eq!(m[0].image_path, "/custom/air.png");

        // no custom -> the Plex thumb
        let m = merge_artists(
            vec![input("Air", 1, 9, "local")],
            &albums,
            &HashMap::new(),
            &plex,
            true,
        );
        assert_eq!(m[0].image_path, "/library/metadata/1/thumb");

        // neither -> the representative album cover, but ONLY with the
        // fallback enabled (Plex on).
        let m = merge_artists(
            vec![input("Air", 1, 9, "local")],
            &albums,
            &HashMap::new(),
            &HashMap::new(),
            true,
        );
        assert_eq!(m[0].image_path, "/covers/air.jpg");
        let m = merge_artists(
            vec![input("Air", 1, 9, "local")],
            &albums,
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        assert_eq!(m[0].image_path, "");
    }

    #[test]
    fn nameless_artists_are_dropped_and_output_is_name_sorted() {
        let merged = merge_artists(
            vec![
                input("zz top", 1, 1, "local"),
                input("   ", 1, 1, "local"),
                input("ABBA", 1, 1, "local"),
            ],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            false,
        );
        let names: Vec<&str> = merged.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["ABBA", "zz top"]);
    }
}
