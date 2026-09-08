//! Location-based artist discovery using MusicBrainz
//!
//! Implements the scene discovery pipeline:
//! 1. Extract artist metadata (location, genres) from MusicBrainz
//! 2. Browse candidates by area with genre affinity scoring
//! 3. Validate candidates against Qobuz catalog

use std::collections::HashSet;

use super::genre::{extract_affinity_seeds, normalize_genre};
use super::{
    AffinitySeeds, Area, ArtistFullResponse, ArtistLocation, ArtistMetadata, ArtistType, LifeSpan,
    LocationPrecision,
};

/// Extract artist metadata from the full MB response
pub fn extract_metadata(response: &ArtistFullResponse) -> ArtistMetadata {
    let artist_type = ArtistType::from(response.artist_type.as_deref());

    // Resolve location: prefer begin_area (city-level), fallback to area (country)
    let location = resolve_location(
        response.begin_area.as_ref(),
        response.area.as_ref(),
        response.country.as_deref(),
    );

    // Extract affinity seeds from tags
    let tags = response.tags.as_deref().unwrap_or(&[]);
    let affinity_seeds = extract_affinity_seeds(tags);

    ArtistMetadata {
        mbid: response.id.clone(),
        name: response.name.clone(),
        artist_type,
        life_span: response.life_span.clone(),
        location,
        affinity_seeds,
    }
}

/// Resolve the most precise location from MB area data
fn resolve_location(
    begin_area: Option<&Area>,
    area: Option<&Area>,
    country: Option<&str>,
) -> Option<ArtistLocation> {
    let cc = country.map(|c| c.to_lowercase());

    // Try begin_area first (formation/birth location — typically city-level)
    if let Some(ba) = begin_area {
        let is_city = ba
            .area_type
            .as_deref()
            .map(|t| t.eq_ignore_ascii_case("city") || t.eq_ignore_ascii_case("municipality"))
            .unwrap_or(false);

        let is_subdivision = ba
            .area_type
            .as_deref()
            .map(|t| t.eq_ignore_ascii_case("subdivision"))
            .unwrap_or(false);

        // MB's "country" field is where the artist is active (not where born).
        // When we have a city-level begin_area, display only the city name
        // to avoid incorrect country attribution (e.g., Zimmer: born Frankfurt,
        // but country=US because he works in the US).
        let precision = if is_city {
            LocationPrecision::City
        } else if is_subdivision {
            LocationPrecision::State
        } else {
            LocationPrecision::City // best guess
        };

        return Some(ArtistLocation {
            city: Some(ba.name.clone()),
            area_id: Some(ba.id.clone()),
            country: country.map(|c| country_code_to_name(c)),
            country_code: cc,
            display_name: ba.name.clone(),
            precision,
        });
    }

    // Fallback to area (usually country-level)
    if let Some(a) = area {
        let is_country = a
            .area_type
            .as_deref()
            .map(|t| t.eq_ignore_ascii_case("country"))
            .unwrap_or(false);

        if is_country {
            return Some(ArtistLocation {
                city: None,
                area_id: Some(a.id.clone()),
                country: Some(a.name.clone()),
                country_code: cc,
                display_name: a.name.clone(),
                precision: LocationPrecision::Country,
            });
        }

        // Non-country area (could be city without begin_area)
        let country_name = country.map(|c| country_code_to_name(c));
        let display = if let Some(ref cn) = country_name {
            format!("{}, {}", a.name, cn)
        } else {
            a.name.clone()
        };

        return Some(ArtistLocation {
            city: Some(a.name.clone()),
            area_id: Some(a.id.clone()),
            country: country_name,
            country_code: cc,
            display_name: display,
            precision: LocationPrecision::City,
        });
    }

    // Country code only (no area data)
    if let Some(raw_cc) = country {
        let name = country_code_to_name(raw_cc);
        return Some(ArtistLocation {
            city: None,
            area_id: None,
            country: Some(name.clone()),
            country_code: cc,
            display_name: name,
            precision: LocationPrecision::Country,
        });
    }

    None
}

/// Convert ISO 3166-1 alpha-2 country code to human-readable name
fn country_code_to_name(code: &str) -> String {
    match code.to_uppercase().as_str() {
        "US" => "United States",
        "GB" => "United Kingdom",
        "CA" => "Canada",
        "AU" => "Australia",
        "DE" => "Germany",
        "FR" => "France",
        "JP" => "Japan",
        "SE" => "Sweden",
        "NO" => "Norway",
        "FI" => "Finland",
        "IE" => "Ireland",
        "NZ" => "New Zealand",
        "BR" => "Brazil",
        "MX" => "Mexico",
        "AR" => "Argentina",
        "CL" => "Chile",
        "CO" => "Colombia",
        "ES" => "Spain",
        "IT" => "Italy",
        "NL" => "Netherlands",
        "BE" => "Belgium",
        "AT" => "Austria",
        "CH" => "Switzerland",
        "DK" => "Denmark",
        "IS" => "Iceland",
        "PT" => "Portugal",
        "PL" => "Poland",
        "CZ" => "Czech Republic",
        "RU" => "Russia",
        "KR" => "South Korea",
        "CN" => "China",
        "TW" => "Taiwan",
        "IN" => "India",
        "ZA" => "South Africa",
        "NG" => "Nigeria",
        "JM" => "Jamaica",
        "CU" => "Cuba",
        "PR" => "Puerto Rico",
        _ => code,
    }
    .to_string()
}

/// Affinity scoring weights
const SCORE_EXACT_CITY: i32 = 40;
const SCORE_SAME_COUNTRY: i32 = 15;
const SCORE_GENRE_CORE: i32 = 20;
const _SCORE_GENRE_SECONDARY: i32 = 10;
const SCORE_TAG_USEFUL: i32 = 8;
const SCORE_NOISY_ONLY: i32 = -12;

/// Compute affinity score for a candidate artist against the source seeds
pub fn compute_affinity_score(
    candidate_tags: &[String],
    source_seeds: &AffinitySeeds,
    same_city: bool,
    same_country: bool,
) -> i32 {
    let mut score: i32 = 0;

    if same_city {
        score += SCORE_EXACT_CITY;
    }
    if same_country {
        score += SCORE_SAME_COUNTRY;
    }

    // Normalize candidate tags for comparison
    let candidate_normalized: HashSet<String> = candidate_tags
        .iter()
        .map(|tag| normalize_genre(tag))
        .collect();

    // Core genre overlap
    let core_overlap = source_seeds
        .genres
        .iter()
        .filter(|g| candidate_normalized.contains(g.as_str()))
        .count();
    score += (core_overlap as i32) * SCORE_GENRE_CORE;

    // Secondary tag overlap
    let tag_overlap = source_seeds
        .tags
        .iter()
        .filter(|tag| candidate_normalized.contains(tag.as_str()))
        .count();
    score += (tag_overlap as i32) * SCORE_TAG_USEFUL;

    // Penalty: if candidate has tags but zero overlap with any seed
    if !candidate_normalized.is_empty() && core_overlap == 0 && tag_overlap == 0 {
        score += SCORE_NOISY_ONLY;
    }

    score
}

/// Build the scene cache key from location + seeds
///
/// UNFIT — kept only so no out-of-tree caller breaks; it has zero callers in
/// this workspace. It carries **only** area + an ordered seed hash, while the
/// response also depends on the source MBID (that artist is excluded from its
/// own scene), the country, the page size and the blacklist. Two callers asking
/// for different page sizes against the same DB would share one entry. Use
/// [`build_scene_cache_key_v1`].
#[deprecated(
    note = "unfit key: ignores source_mbid/country/page_size. Use build_scene_cache_key_v1."
)]
pub fn build_scene_cache_key(area_id: &str, seeds: &AffinitySeeds) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    for seed in &seeds.normalized_seeds {
        seed.hash(&mut hasher);
    }
    let seed_hash = hasher.finish();

    format!("{}:{:x}", area_id, seed_hash)
}

/// Version tag baked into every scene cache key.
///
/// BUMP IT whenever the discovery pipeline changes what a stored
/// `LocationDiscoveryResponse` means — scoring weights, the broad-tag filter,
/// the per-genre limit, the `LocationCandidate` shape. Bumping is how old
/// entries get abandoned; there is no migration and none is wanted, since a
/// stale scene is indistinguishable from a fresh one on the wire.
pub const SCENE_CACHE_KEY_VERSION: &str = "v1";

/// Everything a scene discovery response actually depends on.
///
/// Every field here is load-bearing, which is the whole point — the previous
/// key had two of them:
/// - `source_mbid`: the source artist is *excluded* from their own scene, so
///   two artists from the same city with the same seeds produce different rows.
/// - `area_id`: the MB area id as the caller supplied it, falling back to the
///   area NAME when there is none. Deliberately the pre-resolution identity:
///   city → parent-subdivision resolution is a pure function of this id, so
///   keying on it lets the cache be consulted BEFORE those up-to-5 MB hops are
///   spent. Two cities in one subdivision therefore keep separate entries,
///   which is also what Tauri did.
/// - `country`: the MB query is `area:"<country>"` whenever a country is known,
///   so it, not the subdivision, is what narrows the search.
/// - the seeds: they pick the search genres AND drive the affinity scoring.
/// - `page_size`: a 30-row response must never be served to a 100-row caller.
/// - `catalog_scope`: the Qobuz account territory decides which candidates
///   validate. Currently `None` everywhere (no accessor exists on the core);
///   the slot is here so filling it later is a key change, not a schema change.
///   Until then, invalidate the scene cache on account switch.
pub struct SceneCacheKey<'a> {
    pub source_mbid: &'a str,
    pub area_id: &'a str,
    pub country: Option<&'a str>,
    pub seeds: &'a AffinitySeeds,
    pub page_size: usize,
    pub catalog_scope: Option<&'a str>,
}

/// Build the versioned scene cache key.
///
/// The seed lists are hashed rather than inlined because they are unbounded;
/// everything else stays readable in the key so a human can eyeball the DB.
pub fn build_scene_cache_key_v1(key: &SceneCacheKey<'_>) -> String {
    format!(
        "{}|{}|{}|{}|{:016x}|{}|{}",
        SCENE_CACHE_KEY_VERSION,
        key.source_mbid,
        key.area_id,
        key.country.unwrap_or("-"),
        seed_signature(key.seeds),
        key.page_size,
        key.catalog_scope.unwrap_or("-"),
    )
}

/// Stable 64-bit signature of the affinity seeds.
///
/// Two deliberate choices:
///
/// 1. **Order is preserved**, not sorted. It looks like sorting would give a
///    better hit rate, but the pipeline slices these lists — `genres.chain(
///    tags.take(2))`, and `take(3)` on the all-broad fallback — so a different
///    input order is a genuinely different query. Sorting would collapse two
///    different result sets onto one entry. Duplicates are dropped keeping the
///    FIRST occurrence, which is exactly what `take(n)` sees.
/// 2. **FNV-1a, not `DefaultHasher`.** `DefaultHasher`'s output is explicitly
///    not guaranteed stable across Rust releases; a toolchain bump would
///    silently orphan every cached scene. FNV-1a is eight lines and frozen.
///
/// Genres and tags are hashed as separate sections so `genres=[a] tags=[b]`
/// cannot collide with `genres=[a,b] tags=[]` — they search differently.
fn seed_signature(seeds: &AffinitySeeds) -> u64 {
    let mut buf = String::new();
    push_canonical_list(&mut buf, &seeds.genres);
    buf.push('\u{1e}'); // record separator between the two sections
    push_canonical_list(&mut buf, &seeds.tags);
    fnv1a64(buf.as_bytes())
}

fn push_canonical_list(buf: &mut String, list: &[String]) {
    let mut seen: HashSet<String> = HashSet::new();
    for raw in list {
        let canonical = normalize_genre(raw);
        if canonical.is_empty() || !seen.insert(canonical.clone()) {
            continue;
        }
        buf.push_str(&canonical);
        buf.push('\u{1f}'); // unit separator between entries
    }
}

/// FNV-1a, 64-bit. Deterministic across toolchains, forever, with no dependency.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod scene_key_tests {
    use super::*;

    fn seeds(genres: &[&str], tags: &[&str]) -> AffinitySeeds {
        AffinitySeeds {
            genres: genres.iter().map(|s| s.to_string()).collect(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            normalized_seeds: genres
                .iter()
                .chain(tags.iter())
                .map(|s| s.to_string())
                .collect(),
        }
    }

    fn key(source_mbid: &str, page_size: usize, s: &AffinitySeeds) -> String {
        build_scene_cache_key_v1(&SceneCacheKey {
            source_mbid,
            area_id: "area-1",
            country: Some("United Kingdom"),
            seeds: s,
            page_size,
            catalog_scope: None,
        })
    }

    #[test]
    fn page_size_is_part_of_the_key() {
        // The whole reason R7 was re-decided: Slint asks for 30 rows and Qt
        // asks for 100 against the same DB file.
        let s = seeds(&["post-punk"], &[]);
        assert_ne!(key("mbid-a", 30, &s), key("mbid-a", 100, &s));
    }

    #[test]
    fn source_artist_is_part_of_the_key() {
        // The source artist is excluded from their own scene, so two artists
        // with identical area+seeds do NOT share a response.
        let s = seeds(&["post-punk"], &[]);
        assert_ne!(key("mbid-a", 100, &s), key("mbid-b", 100, &s));
    }

    #[test]
    fn seed_sections_do_not_collide() {
        // genres=[a] tags=[b] searches differently from genres=[a,b] tags=[].
        assert_ne!(
            key("mbid-a", 100, &seeds(&["post-punk"], &["new wave"])),
            key("mbid-a", 100, &seeds(&["post-punk", "new wave"], &[])),
        );
    }

    #[test]
    fn seed_order_is_significant_but_duplicates_are_not() {
        let s = seeds(&["post-punk", "new wave"], &[]);
        assert_ne!(key("mbid-a", 100, &s), key("mbid-a", 100, &seeds(&["new wave", "post-punk"], &[])));
        assert_eq!(
            key("mbid-a", 100, &s),
            key("mbid-a", 100, &seeds(&["post-punk", "new wave", "post-punk"], &[])),
        );
    }
}

/// Format a human-readable date from MB life_span
pub fn format_life_span_date(life_span: &LifeSpan, _is_person: bool) -> Option<String> {
    let begin = life_span.begin.as_deref()?;

    let begin_formatted = format_mb_date(begin);
    let ended = life_span.ended.unwrap_or(false);

    if ended {
        if let Some(end) = life_span.end.as_deref() {
            let end_formatted = format_mb_date(end);
            Some(format!("{}–{}", begin_formatted, end_formatted))
        } else {
            Some(begin_formatted)
        }
    } else {
        Some(begin_formatted)
    }
}

/// Format a MusicBrainz date string into a short human-readable form
/// Input formats: "1990", "1990-05", "1990-05-14"
fn format_mb_date(date: &str) -> String {
    let parts: Vec<&str> = date.split('-').collect();
    match parts.len() {
        1 => parts[0].to_string(),
        2 => {
            let month = match parts[1] {
                "01" => "Jan",
                "02" => "Feb",
                "03" => "Mar",
                "04" => "Apr",
                "05" => "May",
                "06" => "Jun",
                "07" => "Jul",
                "08" => "Aug",
                "09" => "Sep",
                "10" => "Oct",
                "11" => "Nov",
                "12" => "Dec",
                _ => parts[1],
            };
            format!("{} {}", month, parts[0])
        }
        3 => {
            let month = match parts[1] {
                "01" => "Jan",
                "02" => "Feb",
                "03" => "Mar",
                "04" => "Apr",
                "05" => "May",
                "06" => "Jun",
                "07" => "Jul",
                "08" => "Aug",
                "09" => "Sep",
                "10" => "Oct",
                "11" => "Nov",
                "12" => "Dec",
                _ => parts[1],
            };
            format!("{} {}", month, parts[0])
        }
        _ => date.to_string(),
    }
}
