//! Lyrics orchestrator — the Qobuz-first resolution chain (spec §1.1).
//!
//! ```text
//! get(LyricsRequest)
//! ├─ 0. OFFLINE? -> CACHE-ONLY: serve any cached entry (synced or plain);
//! │       miss -> NotAvailableOffline (deviation D3, mission-blessed).
//! ├─ 1. CACHE probe: by track_id, then by cache_key.
//! │       synced hit -> serve. plain-only hit -> SOFT MISS, continue
//! │       (uniform for ALL providers including qobuz — Q4).
//! ├─ 2. QOBUZ (PRIMARY) — only for Qobuz-source tracks with a real track_id:
//! │       wsync -> SYNCED, provider=qobuz -> upsert (+native wsync json,
//! │       amended Q5), serve. DONE.
//! │       plain -> HELD as candidate while LRCLIB is probed for synced
//! │       (the no-sync-regression rule, §1.5).
//! │       miss / any error -> silent degradation, continue.
//! ├─ 3. LRCLIB: search-first + scorer, exactly 1 retry on transport error.
//! │       synced -> serve (provider=lrclib).
//! │       plain-only with a held qobuz-plain -> prefer the QOBUZ plain.
//! ├─ 4. lyrics.ovh (plain-only): only if nothing held so far.
//! └─ 5. UPSERT whatever was served; nothing -> NotFound.
//! ```
//!
//! Fix-forwards baked in: in-flight dedupe keyed by request (F6), request-key
//! echo on every response for the caller's stale guard (F2), typed offline
//! status instead of a hardcoded string (F3), explicit raw-title-to-providers
//! contract (F4 — the `version` field never existed here to be dropped).

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use futures_util::future::{FutureExt, Shared};
use tokio::sync::Mutex;

use qbz_qobuz::{QobuzClient, QobuzLyricsContent, QobuzLyricsDocument};

use crate::cache::{CachedLyrics, LyricsCacheDb, LyricsCacheStats};
use crate::model::{
    build_cache_key, derive_has_translation, LyricsDoc, LyricsPayload, LyricsProvider,
};
use crate::providers::{fetch_lrclib, fetch_lyrics_ovh, LyricsData};
use crate::wsync::{translated_from_content, QobuzWsync};

/// What kind of source the playing track comes from. The Qobuz primary step
/// only applies to Qobuz catalog tracks with a real track_id; local-library /
/// Plex / offline-local tracks go straight to the external fallback chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsSourceKind {
    Qobuz,
    NonQobuz,
}

/// One lyrics lookup request. `offline` is data, not a lookup — the crate
/// never reaches into a frontend's offline store (spec §2.2.4); the glue
/// passes the engine verdict.
#[derive(Debug, Clone)]
pub struct LyricsRequest {
    pub track_id: Option<u64>,
    pub source: LyricsSourceKind,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<u64>,
    pub offline: bool,
    /// Active translation target (ISO 639-1, Qobuz API v10). `None` = the
    /// default original-only fetch; `Some(lang)` asks the Qobuz primary for
    /// a translation and drives the client-derived `has_translation` on the
    /// served document. Only the Qobuz provider is affected — the external
    /// fallback chain never sees it.
    pub language: Option<String>,
}

/// Served lyrics: the wire-compatible payload plus the parsed document
/// (structured lines; native words when the source was Qobuz wsync).
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsResult {
    pub payload: LyricsPayload,
    pub doc: LyricsDoc,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LyricsOutcome {
    Found(LyricsResult),
    NotFound,
    /// Offline and nothing cached — the UI maps this to a translated string
    /// (fix F3; never a hardcoded message).
    NotAvailableOffline,
}

/// Response envelope. `request_track_id`/`request_key` echo the request so
/// callers can discard responses for tracks no longer current (fixes the
/// Tauri stale-response race F2 by construction).
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsResponse {
    pub request_track_id: Option<u64>,
    pub request_key: String,
    pub outcome: LyricsOutcome,
}

/// Provider boundary: the real HTTP lives behind this trait so tests inject
/// fakes and the chain logic stays headless-testable.
#[async_trait]
pub trait LyricsProviders: Send + Sync {
    /// Qobuz primary (two-step: signed lyricsUrl + CDN doc). `language` is the
    /// optional translation target (ISO 639-1, v10); `None` fetches
    /// original-only. `Ok(None)` = typed miss; `Err` = transport/auth/offline
    /// failure — both degrade silently to the fallback chain.
    async fn qobuz(
        &self,
        track_id: u64,
        language: Option<&str>,
    ) -> Result<Option<QobuzLyricsDocument>, String>;

    /// LRCLIB. `Ok(None)` = no match; `Err` = transport error (the chain
    /// retries exactly once).
    async fn lrclib(
        &self,
        title: &str,
        artist: &str,
        duration_secs: Option<u64>,
    ) -> Result<Option<LyricsData>, String>;

    /// lyrics.ovh, plain-only.
    async fn ovh(&self, title: &str, artist: &str) -> Option<LyricsData>;
}

/// Production providers: Qobuz via the shared client, externals via the
/// verbatim-ported fetchers.
pub struct HttpLyricsProviders {
    client: Arc<QobuzClient>,
}

impl HttpLyricsProviders {
    pub fn new(client: Arc<QobuzClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LyricsProviders for HttpLyricsProviders {
    async fn qobuz(
        &self,
        track_id: u64,
        language: Option<&str>,
    ) -> Result<Option<QobuzLyricsDocument>, String> {
        self.client
            .get_lyrics(track_id, language)
            .await
            .map_err(|e| e.to_string())
    }

    async fn lrclib(
        &self,
        title: &str,
        artist: &str,
        duration_secs: Option<u64>,
    ) -> Result<Option<LyricsData>, String> {
        fetch_lrclib(title, artist, duration_secs).await
    }

    async fn ovh(&self, title: &str, artist: &str) -> Option<LyricsData> {
        fetch_lyrics_ovh(title, artist).await
    }
}

struct ServiceInner {
    providers: Arc<dyn LyricsProviders>,
    db: Mutex<Option<LyricsCacheDb>>,
}

type SharedLyricsFuture =
    Shared<Pin<Box<dyn Future<Output = Result<LyricsResponse, String>> + Send>>>;

/// The lyrics service. Owns the per-user cache handle
/// (`init_at`/`teardown`, mirroring Tauri's `LyricsState`) and the in-flight
/// dedupe map; safe to share across tasks/frontends via `Arc`.
pub struct LyricsService {
    inner: Arc<ServiceInner>,
    inflight: StdMutex<HashMap<String, SharedLyricsFuture>>,
}

impl LyricsService {
    pub fn new(providers: Arc<dyn LyricsProviders>) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                providers,
                db: Mutex::new(None),
            }),
            inflight: StdMutex::new(HashMap::new()),
        }
    }

    /// Convenience constructor over the production HTTP providers.
    pub fn with_qobuz_client(client: Arc<QobuzClient>) -> Self {
        Self::new(Arc::new(HttpLyricsProviders::new(client)))
    }

    /// Open the per-user cache at `<base_dir>/lyrics/lyrics.db` — the SAME
    /// file Tauri uses (call on session activation with the user cache dir).
    pub async fn init_at(&self, base_dir: &Path) -> Result<(), String> {
        let cache_dir = base_dir.join("lyrics");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create lyrics cache directory: {}", e))?;
        let db_path = cache_dir.join("lyrics.db");
        let new_db = LyricsCacheDb::new(&db_path)?;
        let mut guard = self.inner.db.lock().await;
        *guard = Some(new_db);
        Ok(())
    }

    /// Drop the cache handle (call on logout).
    pub async fn teardown(&self) {
        let mut guard = self.inner.db.lock().await;
        *guard = None;
    }

    /// Clear the per-user lyrics cache.
    pub async fn clear_cache(&self) -> Result<(), String> {
        let guard = self.inner.db.lock().await;
        let db = guard.as_ref().ok_or(NO_SESSION)?;
        db.clear()
    }

    /// Entries + size, BOTH from the per-user DB (fix F1).
    pub async fn cache_stats(&self) -> Result<LyricsCacheStats, String> {
        let guard = self.inner.db.lock().await;
        let db = guard.as_ref().ok_or(NO_SESSION)?;
        db.stats()
    }

    /// Resolve lyrics for a request through the §1.1 chain. Concurrent calls
    /// for the same request join one in-flight resolution (F6).
    pub async fn get(&self, request: LyricsRequest) -> Result<LyricsResponse, String> {
        let dedupe_key = dedupe_key(&request);

        let (future, is_owner) = {
            let mut inflight = self.inflight.lock().expect("inflight mutex poisoned");
            if let Some(existing) = inflight.get(&dedupe_key) {
                (existing.clone(), false)
            } else {
                let inner = Arc::clone(&self.inner);
                let boxed: Pin<
                    Box<dyn Future<Output = Result<LyricsResponse, String>> + Send>,
                > = Box::pin(run_chain(inner, request));
                let future = boxed.shared();
                inflight.insert(dedupe_key.clone(), future.clone());
                (future, true)
            }
        };

        let result = future.clone().await;

        if is_owner {
            let mut inflight = self.inflight.lock().expect("inflight mutex poisoned");
            if let Some(existing) = inflight.get(&dedupe_key) {
                if existing.ptr_eq(&future) {
                    inflight.remove(&dedupe_key);
                }
            }
        }

        result
    }
}

const NO_SESSION: &str = "No active session - please log in";

fn dedupe_key(request: &LyricsRequest) -> String {
    let identity = match request.track_id {
        Some(id) => format!("id:{}", id),
        None => format!(
            "key:{}",
            build_cache_key(
                request.title.trim(),
                request.artist.trim(),
                request.duration_secs
            )
        ),
    };
    // The translation target is part of the identity: a refetch WITH a
    // language must never join an in-flight original-only resolution.
    format!(
        "{}|offline:{}|lang:{}",
        identity,
        request.offline,
        request.language.as_deref().unwrap_or("")
    )
}

/// A cached entry satisfies a language request only when its stored native
/// wsync document carries a translation FOR THAT language (the content's own
/// `lang` records it, spec §A.5); anything else falls through to a refetch.
/// Original-only requests (`None`) are served by any synced hit, as before.
fn cached_serves_language(cached: &CachedLyrics, language: Option<&str>) -> bool {
    match language {
        None => true,
        Some(lang) => cached
            .qobuz_wsync_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<QobuzWsync>(json).ok())
            .and_then(|wsync| wsync.translation)
            .and_then(|content| content.lang().map(str::to_owned))
            .map(|cached_lang| cached_lang == lang)
            .unwrap_or(false),
    }
}

fn cached_result(cached: CachedLyrics, requested_lang: Option<&str>) -> LyricsResult {
    let doc = LyricsDoc::from_cached(
        &cached.payload,
        cached.qobuz_wsync_json.as_deref(),
        requested_lang,
    );
    LyricsResult {
        payload: cached.payload,
        doc,
    }
}

async fn run_chain(
    inner: Arc<ServiceInner>,
    request: LyricsRequest,
) -> Result<LyricsResponse, String> {
    let title = request.title.trim().to_string();
    let artist = request.artist.trim().to_string();

    if title.is_empty() || artist.is_empty() {
        // Parity with v2_lyrics_get (`legacy_compat.rs:479-486`).
        return Err("Lyrics lookup requires title and artist".to_string());
    }

    let cache_key = build_cache_key(&title, &artist, request.duration_secs);
    let respond = |outcome: LyricsOutcome| LyricsResponse {
        request_track_id: request.track_id,
        request_key: cache_key.clone(),
        outcome,
    };

    // 0. OFFLINE -> cache-only: serve ANY cached entry (synced or plain — no
    //    soft-miss offline, there is nothing to upgrade from); miss -> typed
    //    NotAvailableOffline. External providers are SKIPPED entirely.
    if request.offline {
        let guard = inner.db.lock().await;
        let db = guard.as_ref().ok_or(NO_SESSION)?;
        let cached = match request.track_id {
            Some(id) => db.get_by_track_id(id).ok().flatten(),
            None => None,
        }
        .or_else(|| db.get_by_cache_key(&cache_key).ok().flatten());

        return Ok(match cached {
            Some(cached) => respond(LyricsOutcome::Found(cached_result(
                cached,
                request.language.as_deref(),
            ))),
            None => respond(LyricsOutcome::NotAvailableOffline),
        });
    }

    // 1. Cache probe: by track_id first, then by key. A plain-only entry is a
    //    SOFT MISS and falls through the chain — uniform for every provider
    //    including 'qobuz' (Q4: preserves the synced-upgrade path).
    {
        let guard = inner.db.lock().await;
        let db = guard.as_ref().ok_or(NO_SESSION)?;
        let cached = match request.track_id {
            Some(id) => db.get_by_track_id(id).ok().flatten(),
            None => None,
        }
        .or_else(|| db.get_by_cache_key(&cache_key).ok().flatten());

        if let Some(cached) = cached {
            let has_synced = cached
                .payload
                .synced_lrc
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if has_synced && cached_serves_language(&cached, request.language.as_deref()) {
                return Ok(respond(LyricsOutcome::Found(cached_result(
                    cached,
                    request.language.as_deref(),
                ))));
            }
            // plain-only cache — or a synced entry without a translation for
            // the requested language: fall through to re-fetch (spec §A.5).
        }
    } // lock released before any network round-trip

    let base_payload = |provider: LyricsProvider| LyricsPayload {
        track_id: request.track_id,
        title: title.clone(),
        artist: artist.clone(),
        album: request.album.clone(),
        duration_secs: request.duration_secs,
        plain: None,
        synced_lrc: None,
        provider,
        cached: false,
    };

    // 2+3. QOBUZ primary + LRCLIB run CONCURRENTLY. Running them in parallel
    //       means a Qobuz MISS adds NO latency before LRCLIB (matching Tauri's
    //       speed, which has no Qobuz step) while still preferring Qobuz wsync
    //       when present. The SELECTION below is unchanged from the sequential
    //       version: Qobuz wsync wins outright; otherwise an LRCLIB *synced*
    //       result beats a held Qobuz-plain (no-sync-regression), which beats
    //       an LRCLIB plain-only / miss; lyrics.ovh is the last resort. Qobuz
    //       only runs for Qobuz-source tracks with a real track_id.
    let qobuz_fut = async {
        if request.source == LyricsSourceKind::Qobuz {
            if let Some(track_id) = request.track_id {
                match inner
                    .providers
                    .qobuz(track_id, request.language.as_deref())
                    .await
                {
                    Ok(Some(document)) => return Some(document),
                    Ok(None) => log::debug!("[Lyrics] Qobuz miss for track {}", track_id),
                    Err(e) => {
                        // Offline gate / transport / auth — degrade like a
                        // miss; never a user-facing error (spec §1.2).
                        log::debug!("[Lyrics] Qobuz degraded for track {}: {}", track_id, e)
                    }
                }
            }
        }
        None
    };
    // LRCLIB with exactly 1 retry on transport error (parity:
    // legacy_compat.rs:519-536 — Ok(None) is a miss, NOT a retry).
    let lrclib_fut = async {
        match inner.providers.lrclib(&title, &artist, request.duration_secs).await {
            Ok(data) => data,
            Err(e) => {
                log::warn!("[Lyrics] LRCLIB attempt 1 failed: {}, retrying…", e);
                match inner.providers.lrclib(&title, &artist, request.duration_secs).await {
                    Ok(data) => data,
                    Err(e2) => {
                        log::warn!("[Lyrics] LRCLIB attempt 2 failed: {}, falling back", e2);
                        None
                    }
                }
            }
        }
    };
    let (qobuz_doc, lrclib_data) = tokio::join!(qobuz_fut, lrclib_fut);

    // Apply the Qobuz result: wsync ends the chain; plain is HELD as a
    // candidate while we consider an LRCLIB synced result below.
    let mut qobuz_plain_candidate: Option<LyricsResult> = None;
    if let Some(document) = qobuz_doc {
        if let Some(wsync) = QobuzWsync::from_document(&document) {
            let doc = wsync.to_doc(request.language.as_deref());
            if !doc.lines.is_empty() {
                let mut payload = base_payload(LyricsProvider::Qobuz);
                payload.plain = Some(doc.plain_text());
                payload.synced_lrc = Some(wsync.to_lrc());
                let wsync_json = serde_json::to_string(&wsync)
                    .map_err(|e| format!("Failed to serialize wsync: {}", e))?;

                let guard = inner.db.lock().await;
                let db = guard.as_ref().ok_or(NO_SESSION)?;
                db.upsert(&cache_key, &payload, Some(&wsync_json))?;

                return Ok(respond(LyricsOutcome::Found(LyricsResult { payload, doc })));
            }
        } else if let Some(QobuzLyricsContent::Plain { lines, .. }) = document.original.as_ref() {
            let blob = lines
                .iter()
                .map(|line| line.line.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !blob.trim().is_empty() {
                let mut doc = LyricsDoc::from_plain_text(&blob, LyricsProvider::Qobuz);
                doc.translation_langs = document.translation_langs.clone();
                doc.writers = document.writers.clone();
                doc.translation = document
                    .translation
                    .as_ref()
                    .map(|content| Box::new(translated_from_content(content)));
                doc.has_translation = derive_has_translation(
                    &document.translation_langs,
                    request.language.as_deref(),
                );
                let mut payload = base_payload(LyricsProvider::Qobuz);
                payload.plain = Some(blob.trim().to_string());
                qobuz_plain_candidate = Some(LyricsResult { payload, doc });
            }
        }
    }

    if let Some(data) = lrclib_data {
        let has_synced = data
            .synced_lrc
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        // Plain-only LRCLIB while a qobuz-plain candidate is held -> prefer
        // the QOBUZ plain (first-party, line-split natively — §1.1 step 3).
        if has_synced || qobuz_plain_candidate.is_none() {
            let mut payload = base_payload(data.provider);
            payload.plain = data.plain;
            payload.synced_lrc = data.synced_lrc;
            let doc = LyricsDoc::from_payload(&payload);

            let guard = inner.db.lock().await;
            let db = guard.as_ref().ok_or(NO_SESSION)?;
            db.upsert(&cache_key, &payload, None)?;

            return Ok(respond(LyricsOutcome::Found(LyricsResult { payload, doc })));
        }
    }

    // Held qobuz-plain candidate wins over an LRCLIB plain-only result and
    // over an LRCLIB miss; lyrics.ovh runs only when nothing is held (§1.1).
    if let Some(result) = qobuz_plain_candidate {
        let guard = inner.db.lock().await;
        let db = guard.as_ref().ok_or(NO_SESSION)?;
        db.upsert(&cache_key, &result.payload, None)?;
        return Ok(respond(LyricsOutcome::Found(result)));
    }

    // 4. lyrics.ovh (plain-only fallback #2).
    if let Some(data) = inner.providers.ovh(&title, &artist).await {
        let mut payload = base_payload(data.provider);
        payload.plain = data.plain;
        payload.synced_lrc = data.synced_lrc;
        let doc = LyricsDoc::from_payload(&payload);

        let guard = inner.db.lock().await;
        let db = guard.as_ref().ok_or(NO_SESSION)?;
        db.upsert(&cache_key, &payload, None)?;

        return Ok(respond(LyricsOutcome::Found(LyricsResult { payload, doc })));
    }

    Ok(respond(LyricsOutcome::NotFound))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // ---------- fakes ----------

    #[derive(Default)]
    struct FakeProviders {
        qobuz_queue: StdMutex<VecDeque<Result<Option<QobuzLyricsDocument>, String>>>,
        lrclib_queue: StdMutex<VecDeque<Result<Option<LyricsData>, String>>>,
        ovh_queue: StdMutex<VecDeque<Option<LyricsData>>>,
        qobuz_calls: AtomicUsize,
        lrclib_calls: AtomicUsize,
        ovh_calls: AtomicUsize,
        /// Language argument observed on each qobuz call (threading proof).
        qobuz_languages: StdMutex<Vec<Option<String>>>,
        delay_ms: Option<u64>,
    }

    #[async_trait]
    impl LyricsProviders for FakeProviders {
        async fn qobuz(
            &self,
            _track_id: u64,
            language: Option<&str>,
        ) -> Result<Option<QobuzLyricsDocument>, String> {
            self.qobuz_calls.fetch_add(1, Ordering::SeqCst);
            self.qobuz_languages
                .lock()
                .unwrap()
                .push(language.map(str::to_owned));
            if let Some(ms) = self.delay_ms {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            self.qobuz_queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(None))
        }

        async fn lrclib(
            &self,
            _title: &str,
            _artist: &str,
            _duration_secs: Option<u64>,
        ) -> Result<Option<LyricsData>, String> {
            self.lrclib_calls.fetch_add(1, Ordering::SeqCst);
            self.lrclib_queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(None))
        }

        async fn ovh(&self, _title: &str, _artist: &str) -> Option<LyricsData> {
            self.ovh_calls.fetch_add(1, Ordering::SeqCst);
            self.ovh_queue.lock().unwrap().pop_front().unwrap_or(None)
        }
    }

    fn wsync_document() -> QobuzLyricsDocument {
        serde_json::from_str(
            r#"{
            "track_id": 100, "album_id": "alb",
            "translation_langs": ["es"], "writers": "W. Riter",
            "original": {"type": "wsync", "lang": "en", "lines": [
                {"line": "first line", "start": 1000, "end": 3000,
                 "words": [{"word": "first", "start": 1000, "end": 1800},
                           {"word": "line", "start": 1800, "end": 3000}]},
                {"line": "", "words": []},
                {"line": "second line", "start": 9000, "end": 11000,
                 "words": [{"word": "second", "start": 9000, "end": 9900},
                           {"word": "line", "start": 9900, "end": 11000}]}
            ]}
        }"#,
        )
        .unwrap()
    }

    fn wsync_document_with_translation() -> QobuzLyricsDocument {
        serde_json::from_str(
            r#"{
            "track_id": 100, "album_id": "alb",
            "translation_langs": ["es", "fr"], "writers": "W. Riter",
            "original": {"type": "wsync", "lang": "en", "lines": [
                {"line": "first line", "start": 1000, "end": 3000,
                 "words": [{"word": "first", "start": 1000, "end": 1800},
                           {"word": "line", "start": 1800, "end": 3000}]},
                {"line": "second line", "start": 9000, "end": 11000,
                 "words": [{"word": "second", "start": 9000, "end": 9900},
                           {"word": "line", "start": 9900, "end": 11000}]}
            ]},
            "translation": {"type": "wsync", "lang": "es", "lines": [
                {"line": "primera linea", "start": 1000, "end": 3000,
                 "words": [{"word": "primera", "start": 1000, "end": 1800},
                           {"word": "linea", "start": 1800, "end": 3000}]},
                {"line": "segunda linea", "start": 9000, "end": 11000,
                 "words": [{"word": "segunda", "start": 9000, "end": 9900},
                           {"word": "linea", "start": 9900, "end": 11000}]}
            ]}
        }"#,
        )
        .unwrap()
    }

    fn plain_document() -> QobuzLyricsDocument {
        serde_json::from_str(
            r#"{
            "track_id": 100, "album_id": "alb",
            "original": {"type": "plain", "lang": "en",
                "lines": [{"line": "plain one"}, {"line": "plain two"}]}
        }"#,
        )
        .unwrap()
    }

    fn lrclib_synced() -> LyricsData {
        LyricsData {
            plain: Some("ext one\next two".into()),
            synced_lrc: Some("[00:01.00] ext one\n[00:05.00] ext two".into()),
            provider: LyricsProvider::Lrclib,
        }
    }

    fn lrclib_plain() -> LyricsData {
        LyricsData {
            plain: Some("ext plain".into()),
            synced_lrc: None,
            provider: LyricsProvider::Lrclib,
        }
    }

    fn ovh_plain() -> LyricsData {
        LyricsData {
            plain: Some("ovh plain".into()),
            synced_lrc: None,
            provider: LyricsProvider::Ovh,
        }
    }

    fn request() -> LyricsRequest {
        LyricsRequest {
            track_id: Some(100),
            source: LyricsSourceKind::Qobuz,
            title: "Title".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            duration_secs: Some(200),
            offline: false,
            language: None,
        }
    }

    async fn service_with(providers: Arc<FakeProviders>) -> (LyricsService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let service = LyricsService::new(providers);
        service.init_at(dir.path()).await.unwrap();
        (service, dir)
    }

    fn found(response: &LyricsResponse) -> &LyricsResult {
        match &response.outcome {
            LyricsOutcome::Found(result) => result,
            other => panic!("expected Found, got {:?}", other),
        }
    }

    // ---------- chain priority ----------

    #[tokio::test]
    async fn qobuz_wsync_ends_the_chain() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(wsync_document())));
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert_eq!(result.payload.provider, LyricsProvider::Qobuz);
        assert!(result.doc.synced);
        assert_eq!(result.doc.lines.len(), 2);
        assert!(result.doc.lines[0].words.is_some(), "native words preserved");
        assert_eq!(result.doc.writers.as_deref(), Some("W. Riter"));
        // LRC emitted for cross-frontend compat; plain joined.
        assert!(result.payload.synced_lrc.as_deref().unwrap().contains("[00:01.000] first line"));
        assert_eq!(result.payload.plain.as_deref(), Some("first line\nsecond line"));
        // LRCLIB now runs CONCURRENTLY with Qobuz (parallel resolution), so it
        // is called once — but Qobuz wsync still wins the selection and its
        // result is discarded; ovh never runs once something is found.
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 1);
        assert_eq!(providers.ovh_calls.load(Ordering::SeqCst), 0);
        // Stale-guard echo.
        assert_eq!(response.request_track_id, Some(100));
        assert_eq!(response.request_key, "artist::title::200");
    }

    #[tokio::test]
    async fn qobuz_wsync_is_cached_with_native_column_and_served_from_cache() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(wsync_document())));
        let (service, _dir) = service_with(providers.clone()).await;

        service.get(request()).await.unwrap();

        // Second call: synced cache hit — no provider calls at all.
        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert!(result.payload.cached);
        assert_eq!(result.payload.provider, LyricsProvider::Qobuz);
        // Reader preferred the native wsync column: words survive the cache.
        assert!(result.doc.lines[0].words.is_some());
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 1);
        // LRCLIB ran once on the FIRST call (parallel with Qobuz); the second
        // call was a synced cache hit, so it added no provider traffic.
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn qobuz_plain_held_lrclib_synced_wins() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(plain_document())));
        providers
            .lrclib_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(lrclib_synced())));
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        // No-sync-regression rule: synced LRCLIB beats the held qobuz plain.
        assert_eq!(result.payload.provider, LyricsProvider::Lrclib);
        assert!(result.doc.synced);
        assert_eq!(providers.ovh_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn qobuz_plain_preferred_over_lrclib_plain() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(plain_document())));
        providers
            .lrclib_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(lrclib_plain())));
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert_eq!(result.payload.provider, LyricsProvider::Qobuz);
        assert!(!result.doc.synced);
        assert_eq!(result.payload.plain.as_deref(), Some("plain one\nplain two"));
        // Something was held -> ovh never runs.
        assert_eq!(providers.ovh_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn qobuz_plain_served_when_lrclib_misses_without_touching_ovh() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(plain_document())));
        // lrclib default = Ok(None) miss
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert_eq!(result.payload.provider, LyricsProvider::Qobuz);
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 1);
        assert_eq!(providers.ovh_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn full_fallback_to_ovh_then_not_found() {
        let providers = Arc::new(FakeProviders::default());
        providers.ovh_queue.lock().unwrap().push_back(Some(ovh_plain()));
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert_eq!(result.payload.provider, LyricsProvider::Ovh);
        assert!(!result.doc.synced);

        // ovh result is plain-only -> cached as plain -> soft miss -> chain
        // re-runs on the next call; with every provider missing now, NotFound.
        let response = service.get(request()).await.unwrap();
        assert_eq!(response.outcome, LyricsOutcome::NotFound);
        assert_eq!(response.request_key, "artist::title::200");
    }

    #[tokio::test]
    async fn qobuz_transport_error_degrades_silently() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Err("Offline mode is active".into()));
        providers
            .lrclib_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(lrclib_synced())));
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        assert_eq!(found(&response).payload.provider, LyricsProvider::Lrclib);
    }

    #[tokio::test]
    async fn lrclib_retries_exactly_once_on_transport_error() {
        // Err then Ok(Some) -> retry succeeds.
        let providers = Arc::new(FakeProviders::default());
        {
            let mut q = providers.lrclib_queue.lock().unwrap();
            q.push_back(Err("timeout".into()));
            q.push_back(Ok(Some(lrclib_synced())));
        }
        let (service, _dir) = service_with(providers.clone()).await;
        let response = service.get(request()).await.unwrap();
        assert_eq!(found(&response).payload.provider, LyricsProvider::Lrclib);
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 2);

        // Err, Err -> falls to ovh (exactly 2 attempts, never 3).
        let providers = Arc::new(FakeProviders::default());
        {
            let mut q = providers.lrclib_queue.lock().unwrap();
            q.push_back(Err("timeout".into()));
            q.push_back(Err("timeout".into()));
        }
        providers.ovh_queue.lock().unwrap().push_back(Some(ovh_plain()));
        let (service, _dir) = service_with(providers.clone()).await;
        let response = service.get(request()).await.unwrap();
        assert_eq!(found(&response).payload.provider, LyricsProvider::Ovh);
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 2);
    }

    // ---------- translation (v10) ----------

    #[tokio::test]
    async fn language_threads_to_qobuz_and_translation_is_mapped() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(wsync_document_with_translation())));
        let (service, _dir) = service_with(providers.clone()).await;

        let mut translated = request();
        translated.language = Some("es".into());
        let response = service.get(translated).await.unwrap();
        let result = found(&response);

        // The language reached the provider; the doc carries the mapped
        // translation (wsync -> WordSynced, native word stamps, same timings).
        assert_eq!(
            providers.qobuz_languages.lock().unwrap().as_slice(),
            &[Some("es".to_string())]
        );
        assert!(result.doc.has_translation);
        let translation = result.doc.translation.as_ref().expect("translation mapped");
        assert_eq!(translation.kind, crate::LyricsKind::WordSynced);
        assert_eq!(translation.lang.as_deref(), Some("es"));
        assert_eq!(translation.lines.len(), 2);
        assert_eq!(translation.lines[0].text, "primera linea");
        assert_eq!(translation.lines[0].time_ms, Some(1000));
        let words = translation.lines[0].words.as_ref().expect("words survive");
        assert_eq!(words[0].text, "primera");
        // Sync stays keyed on the ORIGINAL lines/timings.
        assert_eq!(result.doc.lines[0].text, "first line");
    }

    #[tokio::test]
    async fn translation_survives_the_cache_for_the_same_language() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(wsync_document_with_translation())));
        let (service, _dir) = service_with(providers.clone()).await;

        let mut translated = request();
        translated.language = Some("es".into());
        service.get(translated.clone()).await.unwrap();

        // Same language again -> synced cache hit WITH the stored
        // translation; no second provider call.
        let response = service.get(translated).await.unwrap();
        let result = found(&response);
        assert!(result.payload.cached);
        assert!(result.doc.has_translation);
        let translation = result.doc.translation.as_ref().expect("cached translation");
        assert_eq!(translation.lang.as_deref(), Some("es"));
        assert!(translation.lines[0].words.is_some());
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_language_refetches_original_only_uses_the_cache() {
        let providers = Arc::new(FakeProviders::default());
        {
            let mut q = providers.qobuz_queue.lock().unwrap();
            q.push_back(Ok(Some(wsync_document_with_translation())));
            q.push_back(Ok(Some(wsync_document_with_translation())));
        }
        let (service, _dir) = service_with(providers.clone()).await;

        // Seed the cache with the es translation.
        let mut es = request();
        es.language = Some("es".into());
        service.get(es).await.unwrap();

        // A request for a DIFFERENT language misses the stored translation
        // and refetches (spec §A.5: the stored translation records its lang).
        let mut fr = request();
        fr.language = Some("fr".into());
        let response = service.get(fr).await.unwrap();
        assert!(!found(&response).payload.cached);
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 2);

        // Original-only requests are still served by the synced cache hit.
        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert!(result.payload.cached);
        assert!(!result.doc.has_translation);
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 2);
        // The default fetch sent NO language.
        assert_eq!(
            providers.qobuz_languages.lock().unwrap().as_slice(),
            &[Some("es".to_string()), Some("fr".to_string())]
        );
    }

    #[tokio::test]
    async fn default_fetch_maps_original_only_even_when_langs_are_listed() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(wsync_document())));
        let (service, _dir) = service_with(providers.clone()).await;

        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        // translation_langs passthrough present, but nothing was requested:
        // no translation, has_translation false.
        assert_eq!(result.doc.translation_langs, vec!["es"]);
        assert!(result.doc.translation.is_none());
        assert!(!result.doc.has_translation);
        assert_eq!(
            providers.qobuz_languages.lock().unwrap().as_slice(),
            &[None]
        );
    }

    #[tokio::test]
    async fn plain_original_with_plain_translation_is_mapped() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(serde_json::from_str(
                r#"{
                "track_id": 100, "album_id": "alb",
                "translation_langs": ["de"],
                "original": {"type": "plain", "lang": "en",
                    "lines": [{"line": "plain one"}, {"line": "plain two"}]},
                "translation": {"type": "plain", "lang": "de",
                    "lines": [{"line": "einfach eins"}, {"line": "einfach zwei"}]}
            }"#,
            )
            .unwrap())));
        let (service, _dir) = service_with(providers.clone()).await;

        let mut translated = request();
        translated.language = Some("de".into());
        let response = service.get(translated).await.unwrap();
        let result = found(&response);
        assert_eq!(result.payload.provider, LyricsProvider::Qobuz);
        assert!(!result.doc.synced);
        assert!(result.doc.has_translation);
        let translation = result.doc.translation.as_ref().expect("translation mapped");
        assert_eq!(translation.kind, crate::LyricsKind::Plain);
        assert_eq!(translation.lines.len(), 2);
        assert_eq!(translation.lines[0].text, "einfach eins");
    }

    // ---------- cache policy ----------

    #[tokio::test]
    async fn plain_only_cache_is_a_soft_miss_and_upgrades_to_synced() {
        let providers = Arc::new(FakeProviders::default());
        // 1st call: qobuz plain (held + served after lrclib miss).
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(plain_document())));
        let (service, _dir) = service_with(providers.clone()).await;
        let response = service.get(request()).await.unwrap();
        assert_eq!(found(&response).payload.provider, LyricsProvider::Qobuz);
        assert!(!found(&response).doc.synced);

        // 2nd call: the qobuz-plain cache entry soft-misses (uniform rule,
        // Q4) — qobuz misses now, but LRCLIB has synced: upgrade happens.
        providers
            .lrclib_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(lrclib_synced())));
        let response = service.get(request()).await.unwrap();
        let result = found(&response);
        assert_eq!(result.payload.provider, LyricsProvider::Lrclib);
        assert!(result.doc.synced);
        assert!(!result.payload.cached);

        // 3rd call: synced cache hit, no more provider traffic.
        let lrclib_before = providers.lrclib_calls.load(Ordering::SeqCst);
        let response = service.get(request()).await.unwrap();
        assert!(found(&response).payload.cached);
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), lrclib_before);
    }

    #[tokio::test]
    async fn cache_probe_falls_back_from_track_id_to_key() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .lrclib_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(lrclib_synced())));
        let (service, _dir) = service_with(providers.clone()).await;

        // Seed via a request WITHOUT track_id (keyed by metadata only).
        let mut seed = request();
        seed.track_id = None;
        seed.source = LyricsSourceKind::NonQobuz;
        service.get(seed).await.unwrap();

        // Same metadata, now WITH a track_id unknown to the cache: the id
        // probe misses, the key probe hits.
        let response = service.get(request()).await.unwrap();
        assert!(found(&response).payload.cached);
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 1);
    }

    // ---------- offline ----------

    #[tokio::test]
    async fn offline_serves_cached_plain_and_skips_all_providers() {
        let providers = Arc::new(FakeProviders::default());
        providers.ovh_queue.lock().unwrap().push_back(Some(ovh_plain()));
        let (service, _dir) = service_with(providers.clone()).await;

        // Seed a plain-only entry online.
        service.get(request()).await.unwrap();
        let calls_before = (
            providers.qobuz_calls.load(Ordering::SeqCst),
            providers.lrclib_calls.load(Ordering::SeqCst),
            providers.ovh_calls.load(Ordering::SeqCst),
        );

        // Offline: cached PLAIN is served (no soft-miss offline), zero calls.
        let mut offline_request = request();
        offline_request.offline = true;
        let response = service.get(offline_request).await.unwrap();
        let result = found(&response);
        assert!(result.payload.cached);
        assert_eq!(result.payload.provider, LyricsProvider::Ovh);
        assert_eq!(
            (
                providers.qobuz_calls.load(Ordering::SeqCst),
                providers.lrclib_calls.load(Ordering::SeqCst),
                providers.ovh_calls.load(Ordering::SeqCst),
            ),
            calls_before
        );
    }

    #[tokio::test]
    async fn offline_miss_is_typed_not_available_offline() {
        let providers = Arc::new(FakeProviders::default());
        let (service, _dir) = service_with(providers.clone()).await;

        let mut offline_request = request();
        offline_request.offline = true;
        let response = service.get(offline_request).await.unwrap();
        assert_eq!(response.outcome, LyricsOutcome::NotAvailableOffline);
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 0);
        assert_eq!(providers.lrclib_calls.load(Ordering::SeqCst), 0);
        assert_eq!(providers.ovh_calls.load(Ordering::SeqCst), 0);
    }

    // ---------- gating & validation ----------

    #[tokio::test]
    async fn non_qobuz_source_skips_the_qobuz_step() {
        let providers = Arc::new(FakeProviders::default());
        providers
            .lrclib_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(lrclib_synced())));
        let (service, _dir) = service_with(providers.clone()).await;

        let mut local_request = request();
        local_request.source = LyricsSourceKind::NonQobuz;
        let response = service.get(local_request).await.unwrap();
        assert_eq!(found(&response).payload.provider, LyricsProvider::Lrclib);
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_track_id_skips_the_qobuz_step() {
        let providers = Arc::new(FakeProviders::default());
        let (service, _dir) = service_with(providers.clone()).await;

        let mut no_id = request();
        no_id.track_id = None; // Qobuz source but no real id
        let response = service.get(no_id).await.unwrap();
        assert_eq!(response.outcome, LyricsOutcome::NotFound);
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_title_or_artist_is_an_error() {
        let providers = Arc::new(FakeProviders::default());
        let (service, _dir) = service_with(providers).await;

        let mut bad = request();
        bad.title = "   ".into();
        assert!(service.get(bad).await.is_err());

        let mut bad = request();
        bad.artist = String::new();
        assert!(service.get(bad).await.is_err());
    }

    #[tokio::test]
    async fn no_session_is_an_error() {
        let service = LyricsService::new(Arc::new(FakeProviders::default()));
        let err = service.get(request()).await.unwrap_err();
        assert!(err.contains("No active session"));
        assert!(service.cache_stats().await.is_err());
    }

    #[tokio::test]
    async fn teardown_drops_the_cache_handle() {
        let (service, _dir) = service_with(Arc::new(FakeProviders::default())).await;
        assert!(service.cache_stats().await.is_ok());
        service.teardown().await;
        assert!(service.cache_stats().await.is_err());
    }

    // ---------- dedupe (F6) ----------

    #[tokio::test]
    async fn concurrent_same_request_resolves_once() {
        let providers = Arc::new(FakeProviders {
            delay_ms: Some(50),
            ..Default::default()
        });
        providers
            .qobuz_queue
            .lock()
            .unwrap()
            .push_back(Ok(Some(wsync_document())));
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(LyricsService::new(providers.clone()));
        service.init_at(dir.path()).await.unwrap();

        let (a, b) = tokio::join!(service.get(request()), service.get(request()));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(found(&a).payload.provider, LyricsProvider::Qobuz);
        assert_eq!(a, b); // both callers got the one shared resolution
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 1);

        // The in-flight entry is gone afterwards (a later call re-resolves —
        // synced cache hit this time, still no second provider call).
        let c = service.get(request()).await.unwrap();
        assert!(found(&c).payload.cached);
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_requests_do_not_dedupe_against_each_other() {
        let providers = Arc::new(FakeProviders {
            delay_ms: Some(20),
            ..Default::default()
        });
        {
            let mut q = providers.qobuz_queue.lock().unwrap();
            q.push_back(Ok(Some(wsync_document())));
            q.push_back(Ok(Some(wsync_document())));
        }
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(LyricsService::new(providers.clone()));
        service.init_at(dir.path()).await.unwrap();

        let mut other = request();
        other.track_id = Some(200);
        other.title = "Other".into();
        let (a, b) = tokio::join!(service.get(request()), service.get(other));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(providers.qobuz_calls.load(Ordering::SeqCst), 2);
    }
}
