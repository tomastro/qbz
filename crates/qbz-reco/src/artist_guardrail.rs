//! Identity-first Qobuz artist resolution for playlist recommendations.
//!
//! Network access lives behind [`ArtistLookup`] so the acceptance rules can be
//! exercised without Qobuz or MusicBrainz sessions in unit tests.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use qbz_integrations::musicbrainz::cache::{MusicBrainzCache, QobuzArtistMatch};
use qbz_integrations::MusicBrainzClient;
use qbz_models::Artist;
use qbz_qobuz::QobuzClient;
use tokio::sync::RwLock;

/// Boxed future used by the object-safe lookup trait.
pub type ArtistLookupFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The identity and seed-relative facts needed by the recommendation guardrail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistFacts {
    /// Qobuz artist ID.
    pub id: u64,
    /// Catalog artist name.
    pub name: String,
    /// Number of releases advertised by the artist endpoint.
    pub albums_count: u32,
    /// Best available Qobuz artist image.
    pub image: Option<String>,
    /// Level-1 genre ancestor IDs derived from a bounded album sample.
    pub genre_roots: Vec<u64>,
    /// Label IDs derived from the same album sample.
    pub label_ids: Vec<u64>,
}

/// Network/cache seam for artist identity validation.
pub trait ArtistLookup: Send + Sync {
    /// Resolve an asserted Qobuz identity without text search.
    fn artist_by_id(&self, id: u64) -> ArtistLookupFuture<'_, Option<ArtistFacts>>;
    /// Return exact-name candidates for a text fallback.
    fn search_artists(&self, name: &str) -> ArtistLookupFuture<'_, Vec<ArtistFacts>>;
    /// Return the bounded Qobuz neighbourhood for a seed artist.
    fn similar_artist_ids(&self, id: u64) -> ArtistLookupFuture<'_, Vec<u64>>;
    /// Read a confirmed MusicBrainz-name to Qobuz-ID match.
    fn cached_match(&self, name_normalized: &str) -> ArtistLookupFuture<'_, Option<u64>>;
    /// Persist a seed-validated cold-search match.
    fn cache_match(&self, name_normalized: &str, artist: &ArtistFacts);
    /// Fetch normalized MusicBrainz tags for a real MBID.
    fn mb_tags(&self, mbid: &str) -> ArtistLookupFuture<'_, Vec<String>>;
}

/// Production lookup backed by the existing shared clients and optional cache.
pub(crate) struct ProductionArtistLookup {
    qobuz_client: Arc<RwLock<Option<QobuzClient>>>,
    mb_cache: Arc<std::sync::Mutex<Option<MusicBrainzCache>>>,
    mb_client: Arc<MusicBrainzClient>,
}

impl ProductionArtistLookup {
    pub(crate) fn new(
        qobuz_client: Arc<RwLock<Option<QobuzClient>>>,
        mb_cache: Arc<std::sync::Mutex<Option<MusicBrainzCache>>>,
        mb_client: Arc<MusicBrainzClient>,
    ) -> Self {
        Self {
            qobuz_client,
            mb_cache,
            mb_client,
        }
    }

    async fn enrich_artist(client: &QobuzClient, basic: Artist) -> ArtistFacts {
        let detailed = client
            .get_artist_with_pagination_and_locale(basic.id, true, Some(5), None, Some("en"))
            .await;

        let detailed = match detailed {
            Ok(artist) => Some(artist),
            Err(error) => {
                log::warn!(
                    "[SuggestionsEngine] Failed to load seed facts for Qobuz artist {}: {}",
                    basic.id,
                    error
                );
                None
            }
        };

        let albums = detailed
            .as_ref()
            .and_then(|artist| artist.albums.as_ref())
            .map(|albums| albums.items.as_slice())
            .unwrap_or_default();
        let mut genre_roots = Vec::new();
        let mut label_ids = Vec::new();

        for album in albums {
            if let Some(root) = album.genre.as_ref().and_then(|genre| {
                genre.path.as_ref().and_then(|path| match path.as_slice() {
                    [_, level_one, ..] => Some(*level_one),
                    [only] => Some(*only),
                    [] => None,
                })
            }) {
                if !genre_roots.contains(&root) {
                    genre_roots.push(root);
                }
            }
            if let Some(label) = &album.label {
                if !label_ids.contains(&label.id) {
                    label_ids.push(label.id);
                }
            }
        }

        let name = detailed
            .as_ref()
            .map(|artist| artist.name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&basic.name)
            .to_string();
        let albums_count = detailed
            .as_ref()
            .and_then(|artist| artist.albums_count)
            .or_else(|| {
                detailed
                    .as_ref()
                    .and_then(|artist| artist.albums.as_ref())
                    .map(|albums| albums.total)
            })
            .or(basic.albums_count)
            .unwrap_or(0);
        let image = detailed
            .as_ref()
            .and_then(|artist| artist.image.as_ref())
            .or(basic.image.as_ref())
            .and_then(|image| image.best().cloned());

        ArtistFacts {
            id: basic.id,
            name,
            albums_count,
            image,
            genre_roots,
            label_ids,
        }
    }
}

impl ArtistLookup for ProductionArtistLookup {
    fn artist_by_id(&self, id: u64) -> ArtistLookupFuture<'_, Option<ArtistFacts>> {
        Box::pin(async move {
            let guard = self.qobuz_client.read().await;
            let client = guard.as_ref()?;

            // A synthetic qobuz:{id} node is an identity assertion. Resolve
            // that ID directly before enriching it; never turn it into text.
            match client.get_artist_basic(id).await {
                Ok(artist) => Some(Self::enrich_artist(client, artist).await),
                Err(error) => {
                    log::warn!(
                        "[SuggestionsEngine] Failed to resolve Qobuz artist ID {}: {}",
                        id,
                        error
                    );
                    None
                }
            }
        })
    }

    fn search_artists(&self, name: &str) -> ArtistLookupFuture<'_, Vec<ArtistFacts>> {
        let name = name.to_string();
        Box::pin(async move {
            let guard = self.qobuz_client.read().await;
            let Some(client) = guard.as_ref() else {
                return Vec::new();
            };
            let results = match client.search_artists(&name, 10, 0, None).await {
                Ok(results) => results,
                Err(error) => {
                    log::warn!(
                        "[SuggestionsEngine] Artist search failed for '{}': {}",
                        name,
                        error
                    );
                    return Vec::new();
                }
            };

            let normalized = normalize_name(&name);
            let the_variant = format!("the {normalized}");
            let relevant = results.items.into_iter().filter(|artist| {
                let candidate = normalize_name(&artist.name);
                candidate == normalized || candidate == the_variant
            });
            let mut facts = Vec::new();
            for artist in relevant {
                facts.push(Self::enrich_artist(client, artist).await);
            }
            facts
        })
    }

    fn similar_artist_ids(&self, id: u64) -> ArtistLookupFuture<'_, Vec<u64>> {
        Box::pin(async move {
            let guard = self.qobuz_client.read().await;
            let Some(client) = guard.as_ref() else {
                return Vec::new();
            };
            match client.get_similar_artists(id, 20, 0).await {
                Ok(page) => page.items.into_iter().map(|artist| artist.id).collect(),
                Err(error) => {
                    log::warn!(
                        "[SuggestionsEngine] Failed to load similar artists for Qobuz ID {}: {}",
                        id,
                        error
                    );
                    Vec::new()
                }
            }
        })
    }

    fn cached_match(&self, name_normalized: &str) -> ArtistLookupFuture<'_, Option<u64>> {
        let name_normalized = name_normalized.to_string();
        Box::pin(async move {
            let guard = self.mb_cache.lock().ok()?;
            let cache = guard.as_ref()?;
            let cached = cache.get_qobuz_artist_match(&name_normalized).ok()??;
            u64::try_from(cached.qobuz_id).ok().filter(|id| *id > 0)
        })
    }

    fn cache_match(&self, name_normalized: &str, artist: &ArtistFacts) {
        let Ok(guard) = self.mb_cache.lock() else {
            return;
        };
        let Some(cache) = guard.as_ref() else {
            return;
        };
        let Ok(qobuz_id) = i64::try_from(artist.id) else {
            return;
        };
        if let Err(error) = cache.set_qobuz_artist_match(
            name_normalized,
            &QobuzArtistMatch {
                qobuz_id,
                name: artist.name.clone(),
                image: artist.image.clone(),
                albums_count: Some(artist.albums_count),
            },
        ) {
            log::warn!(
                "[SuggestionsEngine] Failed to cache Qobuz artist match '{}': {}",
                artist.name,
                error
            );
        }
    }

    fn mb_tags(&self, mbid: &str) -> ArtistLookupFuture<'_, Vec<String>> {
        let mbid = mbid.to_string();
        Box::pin(async move {
            match self.mb_client.get_artist_tags(&mbid).await {
                Ok(tags) => tags,
                Err(error) => {
                    log::debug!(
                        "[SuggestionsEngine] MusicBrainz tags unavailable for {}: {}",
                        mbid,
                        error
                    );
                    Vec::new()
                }
            }
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSeed {
    pub(crate) node_id: String,
    pub(crate) requested_name: String,
    pub(crate) facts: ArtistFacts,
}

/// Qobuz and MusicBrainz context collected once from the playlist seeds.
#[derive(Debug, Clone, Default)]
pub struct SeedContext {
    seeds: Vec<ResolvedSeed>,
    genre_roots: HashSet<u64>,
    label_ids: HashSet<u64>,
    tags: HashSet<String>,
    neighbourhood_ids: Vec<u64>,
    neighbourhood_set: HashSet<u64>,
}

impl SeedContext {
    pub(crate) fn resolved_seed(
        &self,
        node_id: &str,
        requested_name: &str,
    ) -> Option<&ArtistFacts> {
        self.seeds
            .iter()
            .find(|seed| {
                seed.node_id == node_id
                    && normalize_name(&seed.requested_name) == normalize_name(requested_name)
            })
            .map(|seed| &seed.facts)
    }

    pub(crate) fn neighbourhood_ids(&self) -> &[u64] {
        &self.neighbourhood_ids
    }

    async fn refresh_qobuz_context(&mut self, lookup: &dyn ArtistLookup) {
        self.genre_roots.clear();
        self.label_ids.clear();
        self.neighbourhood_ids.clear();
        self.neighbourhood_set.clear();

        let mut seen_seed_ids = HashSet::new();
        for seed in &self.seeds {
            self.genre_roots
                .extend(seed.facts.genre_roots.iter().copied());
            self.label_ids.extend(seed.facts.label_ids.iter().copied());
            if !seen_seed_ids.insert(seed.facts.id) {
                continue;
            }
            for similar_id in lookup.similar_artist_ids(seed.facts.id).await {
                if self.neighbourhood_set.insert(similar_id) {
                    self.neighbourhood_ids.push(similar_id);
                }
            }
        }
    }
}

/// Build the seed-relative validation context once per suggestion request.
pub async fn resolve_seed_context(
    lookup: &dyn ArtistLookup,
    playlist_artists: &[(String, String)],
) -> SeedContext {
    let mut context = SeedContext::default();
    let mut ambiguous = Vec::new();

    for (node_id, requested_name) in playlist_artists {
        let qobuz_id = parse_qobuz_id(node_id);
        let mut seed_has_tags = false;
        if qobuz_id.is_none() {
            for tag in lookup.mb_tags(node_id).await {
                let tag = normalize_tag(&tag);
                if !tag.is_empty() {
                    seed_has_tags = true;
                    context.tags.insert(tag);
                }
            }
        }

        if let Some(qobuz_id) = qobuz_id {
            if let Some(facts) = lookup.artist_by_id(qobuz_id).await {
                if has_expected_identity(&facts, requested_name)
                    && seed_has_identity_signal(&facts, seed_has_tags)
                {
                    context.seeds.push(ResolvedSeed {
                        node_id: node_id.clone(),
                        requested_name: requested_name.clone(),
                        facts,
                    });
                }
            }
            continue;
        }

        let cache_key = MusicBrainzCache::normalize_name(requested_name);
        if let Some(qobuz_id) = lookup.cached_match(&cache_key).await {
            if let Some(facts) = lookup.artist_by_id(qobuz_id).await {
                if has_expected_identity(&facts, requested_name)
                    && seed_has_identity_signal(&facts, seed_has_tags)
                {
                    context.seeds.push(ResolvedSeed {
                        node_id: node_id.clone(),
                        requested_name: requested_name.clone(),
                        facts,
                    });
                }
            }
            // A cache hit is authoritative enough to avoid a text fallback;
            // a bad/stale row is rejected instead of silently changing IDs.
            continue;
        }

        let search = named_search_candidates(lookup, requested_name).await;
        if search.candidates.len() == 1
            && !search.homonymous
            && seed_has_identity_signal(&search.candidates[0], seed_has_tags)
        {
            context.seeds.push(ResolvedSeed {
                node_id: node_id.clone(),
                requested_name: requested_name.clone(),
                facts: search.candidates.into_iter().next().expect("one candidate"),
            });
        } else if !search.candidates.is_empty() {
            ambiguous.push((node_id.clone(), requested_name.clone(), search.candidates));
        }
    }

    // Definite seed identities establish the neighbourhood used to resolve any
    // same-name seed ambiguity. With no such evidence, ambiguity stays rejected.
    if !ambiguous.is_empty() {
        context.refresh_qobuz_context(lookup).await;
    }
    for (node_id, requested_name, candidates) in ambiguous {
        let mut validated = Vec::new();
        for candidate in candidates {
            if validate_candidate(lookup, &candidate, Some(&node_id), &context).await {
                validated.push(candidate);
            }
        }
        if let Some(facts) = select_homonym(validated, &context) {
            context.seeds.push(ResolvedSeed {
                node_id,
                requested_name,
                facts,
            });
        }
    }
    context.refresh_qobuz_context(lookup).await;
    context
}

/// Resolve a related-artist node, then apply the seed-relative guardrail.
pub async fn resolve_candidate(
    lookup: &dyn ArtistLookup,
    node_id: &str,
    requested_name: &str,
    seed_context: &SeedContext,
) -> Option<ArtistFacts> {
    if let Some(qobuz_id) = parse_qobuz_id(node_id) {
        let facts = lookup.artist_by_id(qobuz_id).await?;
        if !has_expected_identity(&facts, requested_name) {
            return None;
        }
        return validate_candidate(lookup, &facts, None, seed_context)
            .await
            .then_some(facts);
    }

    let cache_key = MusicBrainzCache::normalize_name(requested_name);
    if let Some(qobuz_id) = lookup.cached_match(&cache_key).await {
        let facts = lookup.artist_by_id(qobuz_id).await?;
        if !has_expected_identity(&facts, requested_name) {
            return None;
        }
        return validate_candidate(lookup, &facts, Some(node_id), seed_context)
            .await
            .then_some(facts);
    }

    let search = named_search_candidates(lookup, requested_name).await;
    let mut validated = Vec::new();
    for candidate in search.candidates {
        if validate_candidate(lookup, &candidate, Some(node_id), seed_context).await {
            validated.push(candidate);
        }
    }
    let facts = if search.homonymous {
        select_homonym(validated, seed_context)?
    } else {
        validated.into_iter().next()?
    };

    // Only seed-relative validation turns a cold text hit into a confirmed
    // reusable identity match.
    lookup.cache_match(&cache_key, &facts);
    Some(facts)
}

/// Accept only candidates sharing a Qobuz genre root or MusicBrainz tag with a seed.
pub async fn validate_candidate(
    lookup: &dyn ArtistLookup,
    candidate: &ArtistFacts,
    candidate_mbid: Option<&str>,
    seed_context: &SeedContext,
) -> bool {
    if candidate.albums_count == 0 {
        return false;
    }

    if candidate
        .genre_roots
        .iter()
        .any(|root| seed_context.genre_roots.contains(root))
    {
        return true;
    }

    let candidate_tags = if let Some(mbid) = candidate_mbid {
        lookup
            .mb_tags(mbid)
            .await
            .into_iter()
            .map(|tag| normalize_tag(&tag))
            .filter(|tag| !tag.is_empty())
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    if candidate_tags
        .iter()
        .any(|tag| seed_context.tags.contains(tag))
    {
        return true;
    }

    if candidate.genre_roots.is_empty() && candidate_tags.is_empty() {
        log::debug!(
            "[SuggestionsEngine] Rejecting '{}' because identity context is unavailable",
            candidate.name
        );
    }
    false
}

struct NamedSearchCandidates {
    candidates: Vec<ArtistFacts>,
    homonymous: bool,
}

async fn named_search_candidates(
    lookup: &dyn ArtistLookup,
    requested_name: &str,
) -> NamedSearchCandidates {
    let normalized = normalize_name(requested_name);
    let mut results = lookup.search_artists(requested_name).await;
    if results.is_empty() && requested_name != normalized {
        results = lookup.search_artists(&normalized).await;
    }

    let mut exact = matching_candidates(&results, &normalized);
    if exact.candidates.is_empty() {
        exact = matching_candidates(&results, &format!("the {normalized}"));
    }
    exact
}

fn matching_candidates(results: &[ArtistFacts], normalized_name: &str) -> NamedSearchCandidates {
    let mut seen = HashSet::new();
    let matches = results
        .iter()
        .filter(|artist| normalize_name(&artist.name) == normalized_name)
        .filter(|artist| seen.insert(artist.id))
        .cloned()
        .collect::<Vec<_>>();
    NamedSearchCandidates {
        homonymous: matches.len() > 1,
        candidates: matches
            .into_iter()
            .filter(|artist| artist.albums_count > 0)
            .collect(),
    }
}

fn select_homonym(candidates: Vec<ArtistFacts>, context: &SeedContext) -> Option<ArtistFacts> {
    candidates.into_iter().find(|candidate| {
        candidate.albums_count > 0
            && (context.neighbourhood_set.contains(&candidate.id)
                || candidate
                    .label_ids
                    .iter()
                    .any(|label| context.label_ids.contains(label)))
    })
}

fn has_expected_identity(artist: &ArtistFacts, requested_name: &str) -> bool {
    if artist.albums_count == 0 {
        return false;
    }
    let requested = normalize_name(requested_name);
    let actual = normalize_name(&artist.name);
    actual == requested || actual == format!("the {requested}")
}

fn seed_has_identity_signal(artist: &ArtistFacts, has_tags: bool) -> bool {
    !artist.genre_roots.is_empty() || has_tags
}

fn parse_qobuz_id(node_id: &str) -> Option<u64> {
    node_id.strip_prefix("qobuz:")?.parse().ok()
}

/// Normalize a name for identity comparisons (accent-insensitive, as before).
pub(crate) fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .replace('á', "a")
        .replace('é', "e")
        .replace('í', "i")
        .replace('ó', "o")
        .replace('ú', "u")
        .replace('à', "a")
        .replace('è', "e")
        .replace('ì', "i")
        .replace('ò', "o")
        .replace('ù', "u")
        .replace('ä', "a")
        .replace('ë', "e")
        .replace('ï', "i")
        .replace('ö', "o")
        .replace('ü', "u")
        .replace('ñ', "n")
        .replace('ç', "c")
}

fn normalize_tag(tag: &str) -> String {
    tag.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeLookup {
        artists: HashMap<u64, ArtistFacts>,
        searches: HashMap<String, Vec<ArtistFacts>>,
        similar: HashMap<u64, Vec<u64>>,
        cached: std::sync::Mutex<HashMap<String, u64>>,
        tags: HashMap<String, Vec<String>>,
        artist_by_id_calls: AtomicUsize,
        search_calls: AtomicUsize,
    }

    impl ArtistLookup for FakeLookup {
        fn artist_by_id(&self, id: u64) -> ArtistLookupFuture<'_, Option<ArtistFacts>> {
            self.artist_by_id_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { self.artists.get(&id).cloned() })
        }

        fn search_artists(&self, name: &str) -> ArtistLookupFuture<'_, Vec<ArtistFacts>> {
            self.search_calls.fetch_add(1, Ordering::SeqCst);
            let name = name.to_string();
            Box::pin(async move { self.searches.get(&name).cloned().unwrap_or_default() })
        }

        fn similar_artist_ids(&self, id: u64) -> ArtistLookupFuture<'_, Vec<u64>> {
            Box::pin(async move { self.similar.get(&id).cloned().unwrap_or_default() })
        }

        fn cached_match(&self, name_normalized: &str) -> ArtistLookupFuture<'_, Option<u64>> {
            let hit = self
                .cached
                .lock()
                .ok()
                .and_then(|cache| cache.get(name_normalized).copied());
            Box::pin(async move { hit })
        }

        fn cache_match(&self, name_normalized: &str, artist: &ArtistFacts) {
            if let Ok(mut cache) = self.cached.lock() {
                cache.insert(name_normalized.to_string(), artist.id);
            }
        }

        fn mb_tags(&self, mbid: &str) -> ArtistLookupFuture<'_, Vec<String>> {
            let mbid = mbid.to_string();
            Box::pin(async move { self.tags.get(&mbid).cloned().unwrap_or_default() })
        }
    }

    fn facts(id: u64, name: &str, roots: &[u64], labels: &[u64]) -> ArtistFacts {
        ArtistFacts {
            id,
            name: name.to_string(),
            albums_count: 1,
            image: None,
            genre_roots: roots.to_vec(),
            label_ids: labels.to_vec(),
        }
    }

    fn context(root: u64) -> SeedContext {
        SeedContext {
            genre_roots: HashSet::from([root]),
            ..SeedContext::default()
        }
    }

    #[tokio::test]
    async fn qobuz_node_resolves_by_id_without_text_search() {
        let artist = facts(42, "Direct Artist", &[7], &[]);
        let lookup = FakeLookup {
            artists: HashMap::from([(42, artist.clone())]),
            ..FakeLookup::default()
        };

        let resolved = resolve_candidate(&lookup, "qobuz:42", "Direct Artist", &context(7)).await;

        assert_eq!(resolved, Some(artist));
        assert_eq!(lookup.artist_by_id_calls.load(Ordering::SeqCst), 1);
        assert_eq!(lookup.search_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn mbid_uses_cache_before_search_and_searches_only_on_miss() {
        let cached_artist = facts(10, "Cached Artist", &[7], &[]);
        let cached_lookup = FakeLookup {
            artists: HashMap::from([(10, cached_artist.clone())]),
            cached: std::sync::Mutex::new(HashMap::from([(
                MusicBrainzCache::normalize_name("Cached Artist"),
                10,
            )])),
            ..FakeLookup::default()
        };

        assert_eq!(
            resolve_candidate(&cached_lookup, "real-mbid", "Cached Artist", &context(7)).await,
            Some(cached_artist)
        );
        assert_eq!(cached_lookup.search_calls.load(Ordering::SeqCst), 0);

        let searched_artist = facts(20, "Searched Artist", &[7], &[]);
        let miss_lookup = FakeLookup {
            searches: HashMap::from([(
                "Searched Artist".to_string(),
                vec![searched_artist.clone()],
            )]),
            ..FakeLookup::default()
        };
        assert_eq!(
            resolve_candidate(&miss_lookup, "another-mbid", "Searched Artist", &context(7)).await,
            Some(searched_artist)
        );
        assert_eq!(miss_lookup.search_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn homonym_requires_seed_neighbourhood_or_label_evidence() {
        let wrong = facts(10, "Same Name", &[7], &[]);
        let neighbour = facts(11, "Same Name", &[7], &[]);
        let lookup = FakeLookup {
            searches: HashMap::from([("Same Name".to_string(), vec![wrong, neighbour.clone()])]),
            similar: HashMap::from([(1, vec![11])]),
            ..FakeLookup::default()
        };
        let mut seed_context = SeedContext {
            seeds: vec![ResolvedSeed {
                node_id: "seed-mbid".to_string(),
                requested_name: "Seed Artist".to_string(),
                facts: facts(1, "Seed Artist", &[7], &[]),
            }],
            ..SeedContext::default()
        };
        seed_context.refresh_qobuz_context(&lookup).await;

        assert_eq!(
            resolve_candidate(&lookup, "candidate-mbid", "Same Name", &seed_context).await,
            Some(neighbour)
        );

        let discarded_lookup = FakeLookup {
            searches: lookup.searches.clone(),
            ..FakeLookup::default()
        };
        assert_eq!(
            resolve_candidate(
                &discarded_lookup,
                "candidate-mbid",
                "Same Name",
                &context(7),
            )
            .await,
            None
        );
    }

    #[tokio::test]
    async fn homonym_validation_precedes_neighbourhood_tie_breaking() {
        let invalid_neighbour = facts(11, "Same Name", &[8], &[]);
        let valid_label_match = facts(12, "Same Name", &[7], &[50]);
        let lookup = FakeLookup {
            searches: HashMap::from([(
                "Same Name".to_string(),
                vec![invalid_neighbour, valid_label_match.clone()],
            )]),
            ..FakeLookup::default()
        };
        let mut seed_context = context(7);
        seed_context.neighbourhood_set.insert(11);
        seed_context.label_ids.insert(50);

        assert_eq!(
            resolve_candidate(&lookup, "candidate-mbid", "Same Name", &seed_context).await,
            Some(valid_label_match)
        );
    }

    #[tokio::test]
    async fn candidate_needs_shared_genre_root_or_musicbrainz_tag() {
        let lookup = FakeLookup::default();
        assert!(
            validate_candidate(
                &lookup,
                &facts(10, "Shared Root", &[7], &[]),
                None,
                &context(7),
            )
            .await
        );
        assert!(
            !validate_candidate(
                &lookup,
                &facts(11, "Different Root", &[8], &[]),
                None,
                &context(7),
            )
            .await
        );
        assert!(
            !validate_candidate(
                &lookup,
                &facts(12, "No Signals", &[], &[]),
                None,
                &context(7),
            )
            .await
        );

        let tagged_lookup = FakeLookup {
            tags: HashMap::from([("candidate-mbid".to_string(), vec!["Metal".to_string()])]),
            ..FakeLookup::default()
        };
        let mut tagged_context = context(7);
        tagged_context.tags.insert("metal".to_string());
        assert!(
            validate_candidate(
                &tagged_lookup,
                &facts(13, "Tagged Artist", &[8], &[]),
                Some("candidate-mbid"),
                &tagged_context,
            )
            .await
        );
    }
}
