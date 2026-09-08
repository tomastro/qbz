//! My QBZ — DJ-mix "Random queue" sampler. The Qt port of
//! `crates/qbz/src/myqbz_mix.rs` (spec 02 §6.6), publishing
//! `QbzMyQbz.mixJson` (spec 02 §5.6).
//!
//! ## Data path
//!
//! - **open**: publish `{open:true, loading:true, …}` immediately, load the
//!   collection on a blocking worker, resolve its items **IN ORDER**
//!   (`play_mode` is IGNORED for DJ-mix — the sampler does its own
//!   randomization), then run the DETERMINISTIC
//!   `qbz_mixtape::shuffle::unique_track_count` for the slider max. The discrete
//!   size set is [`build_size_options`]; the slider indexes into it.
//! - **shuffle**: re-resolve in order, then — in a SYNC scope that ends BEFORE
//!   any `.await` — run `dedup_by_similarity` + `hybrid_sample`. The sampled
//!   queue goes to `myqbz_play_qt::play_all_tracks` (replace + start at 0 +
//!   queue-source stamp + best-effort `touch_play`).
//!
//! ## RNG confinement (load-bearing, spec 02 §7 T2)
//!
//! `rand::rng()` returns a `ThreadRng`, which is `!Send`. Holding it across an
//! `.await` makes the spawned future non-`Send` and FAILS TO COMPILE. So the
//! resolve `.await` completes FIRST into an owned `Vec<QueueTrack>`; the
//! dedup + sample then run inside a plain synchronous block that creates, uses
//! and DROPS the RNG entirely before the next `.await`.
//!
//! The sampler constants (`SIMILARITY_THRESHOLD` 0.80, `ALBUM_CAP_PCT` 0.30,
//! `ALBUM_CAP_MIN` 2) live in `qbz-mixtape` and are NOT duplicated here.

use cxx_qt_lib::QString;
use qbz_models::QueueTrack;
use serde::Serialize;

/// Below this unique-count the only option is a single "All (N)" entry.
const SMALL_THRESHOLD: i32 = 50;
/// Intermediate options step (50, 100, 150, …).
const STEP: i32 = 50;

// ---------------------------------------------------------------------------
// Document (spec 02 §5.6)
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize)]
pub struct MixDoc {
    pub open: bool,
    pub loading: bool,
    #[serde(rename = "uniqueCount")]
    pub unique_count: i32,
    #[serde(rename = "sizeOptions")]
    pub size_options: Vec<i32>,
    #[serde(rename = "selectedIndex")]
    pub selected_index: i32,
    #[serde(rename = "selectedSize")]
    pub selected_size: i32,
    #[serde(rename = "selectedIsAll")]
    pub selected_is_all: bool,
    pub busy: bool,
}

static DOC: std::sync::Mutex<Option<MixDoc>> = std::sync::Mutex::new(None);

fn with_doc<R>(f: impl FnOnce(&mut MixDoc) -> R) -> R {
    let mut guard = DOC.lock().unwrap();
    let doc = guard.get_or_insert_with(MixDoc::default);
    f(doc)
}

fn publish(doc: &MixDoc) {
    let json = serde_json::to_string(doc).unwrap_or_else(|_| "{}".into());
    crate::myqbz_bridge::ui(move |mut b| {
        b.as_mut().set_mix_json(QString::from(json.as_str()));
    });
}

fn publish_current() {
    let doc = with_doc(|d| d.clone());
    publish(&doc);
}

// ---------------------------------------------------------------------------
// Size options
// ---------------------------------------------------------------------------

/// The discrete size options for `unique_count`. The LAST entry is ALWAYS the
/// "All (N)" option, so `index == len - 1` ⇒ "All".
///
/// - `<= 0` ⇒ `[]` (the modal stays empty)
/// - `< 50` ⇒ `[unique_count]` (one "All (N)" entry)
/// - else ⇒ `[50,100,150,…]` for each `s < unique_count`, then a trailing
///   `unique_count`. The loop stops STRICTLY below, so a multiple of 50 does not
///   duplicate: `100` ⇒ `[50, 100]`, never `[50, 100, 100]`.
pub(crate) fn build_size_options(unique_count: i32) -> Vec<i32> {
    if unique_count <= 0 {
        return Vec::new();
    }
    if unique_count < SMALL_THRESHOLD {
        return vec![unique_count];
    }
    let mut out = Vec::new();
    let mut s = STEP;
    while s < unique_count {
        out.push(s);
        s += STEP;
    }
    out.push(unique_count); // trailing "All (N)".
    out
}

/// Set the slider index and derive the selected size + is-all flag. Clamps to
/// the valid range; an empty option set zeroes everything.
pub(crate) fn apply_index(index: i32) {
    let doc = with_doc(|d| {
        let len = d.size_options.len() as i32;
        if len == 0 {
            d.selected_index = 0;
            d.selected_size = 0;
            d.selected_is_all = false;
        } else {
            let idx = index.clamp(0, len - 1);
            d.selected_index = idx;
            d.selected_size = d.size_options.get(idx as usize).copied().unwrap_or(0);
            // The trailing entry is always the "All (N)" option.
            d.selected_is_all = idx == len - 1;
        }
        d.clone()
    });
    publish(&doc);
}

/// Push the resolved options for `unique_count`, defaulting the selection to
/// index 0 (= 50 for a large collection, or the single "All (N)" entry).
fn apply_options(unique_count: i32) {
    with_doc(|d| {
        d.unique_count = unique_count;
        d.size_options = build_size_options(unique_count);
        d.loading = false;
    });
    apply_index(0);
}

// ---------------------------------------------------------------------------
// open / close
// ---------------------------------------------------------------------------

/// Close the modal + clear its flags.
pub(crate) fn close() {
    let doc = with_doc(|d| {
        d.open = false;
        d.busy = false;
        d.loading = false;
        d.clone()
    });
    publish(&doc);
}

fn close_with_error(msg: String) {
    close();
    crate::toast_qt::error(msg);
}

/// Drop the modal state on logout.
pub(crate) fn teardown() {
    let doc = with_doc(|d| {
        *d = MixDoc::default();
        d.clone()
    });
    publish(&doc);
}

/// Open the DJ-mix modal for the collection on screen: show it in its
/// "computing…" state, resolve in order on a worker, count unique tracks
/// (deterministic, no RNG) and fill the slider. A resolve that yields nothing
/// closes the modal with an error toast.
pub(crate) fn open() {
    let collection_id = crate::myqbz_detail_qt::current_id();
    if collection_id.is_empty() {
        return;
    }
    // Show the modal immediately in its loading state.
    with_doc(|d| {
        d.loading = true;
        d.busy = false;
        d.unique_count = 0;
        d.size_options = Vec::new();
        d.selected_index = 0;
        d.selected_size = 0;
        d.selected_is_all = false;
        d.open = true;
    });
    publish_current();

    let runtime = crate::app();
    crate::spawn(async move {
        let Some(collection) = crate::myqbz_play_qt::load_collection(&collection_id).await else {
            close_with_error(qbz_i18n::t("Couldn't load this collection"));
            return;
        };
        // Always InOrder for DJ-mix (force_shuffle = false): the sampler does
        // its own randomization; the resolve only needs the full track pool.
        let tracks = crate::myqbz_play_qt::resolve_collection(&runtime, &collection, false).await;
        let unique = qbz_mixtape::shuffle::unique_track_count(&tracks) as i32;
        if unique <= 0 {
            close();
            crate::toast_qt::error(qbz_i18n::t("This collection resolved to 0 playable tracks"));
        } else {
            apply_options(unique);
        }
    });
}

// ---------------------------------------------------------------------------
// confirm / sample
// ---------------------------------------------------------------------------

/// Confirm: sample `sample_size` songs and replace-play the queue. The modal
/// closes BEFORE playback starts (1:1 with `handleConfirmMix`). When the sampled
/// count is below the requested one (the per-album cap can shrink the pool) an
/// info toast reports it.
pub(crate) fn confirm() {
    let sample_size = with_doc(|d| d.selected_size);
    shuffle(sample_size);
}

/// The confirm body, taking the size explicitly so a caller that already knows
/// it does not have to round-trip through the document.
pub(crate) fn shuffle(sample_size: i32) {
    let collection_id = crate::myqbz_detail_qt::current_id();
    if collection_id.is_empty() || sample_size <= 0 {
        return;
    }
    // Disable the Shuffle button while in flight.
    with_doc(|d| d.busy = true);
    publish_current();

    let runtime = crate::app();
    crate::spawn(async move {
        let Some(collection) = crate::myqbz_play_qt::load_collection(&collection_id).await else {
            close_with_error(qbz_i18n::t("Couldn't load this collection"));
            return;
        };
        // Resolve in order — this `.await` completes BEFORE the RNG exists.
        let resolved = crate::myqbz_play_qt::resolve_collection(&runtime, &collection, false).await;

        // ── RNG-confined sync scope (T2): create, use and DROP the !Send
        // ThreadRng entirely here — it never crosses the `.await` below.
        let requested = sample_size as usize;
        let sampled: Vec<QueueTrack> = {
            let mut rng = rand::rng();
            let deduped = qbz_mixtape::shuffle::dedup_by_similarity(resolved, &mut rng);
            qbz_mixtape::shuffle::hybrid_sample(deduped, requested, &mut rng)
        };
        // `rng` is dropped; from here on the future is `Send` again.

        let actual = sampled.len();

        // Close before playback starts.
        close();

        // Replace-play: set_queue + start at 0 + queue-source stamp +
        // best-effort touch_play.
        crate::myqbz_play_qt::play_all_tracks(&runtime, &collection_id, sampled).await;

        // The per-album cap can shrink the pool below the request — surface it.
        if actual > 0 && actual < requested {
            crate::toast_qt::info(qbz_i18n::t_args(
                "Playing {} of {}",
                &[&actual.to_string(), &requested.to_string()],
            ));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_options_empty_for_non_positive_counts() {
        assert!(build_size_options(0).is_empty());
        assert!(build_size_options(-5).is_empty());
    }

    #[test]
    fn size_options_small_collection_is_one_all_entry() {
        assert_eq!(build_size_options(1), vec![1]);
        assert_eq!(build_size_options(49), vec![49]);
    }

    #[test]
    fn size_options_multiple_of_fifty_does_not_duplicate() {
        assert_eq!(build_size_options(50), vec![50]);
        assert_eq!(build_size_options(100), vec![50, 100]);
        assert_eq!(build_size_options(150), vec![50, 100, 150]);
    }

    #[test]
    fn size_options_trailing_entry_is_always_the_total() {
        let opts = build_size_options(137);
        assert_eq!(opts, vec![50, 100, 137]);
        assert_eq!(*opts.last().unwrap(), 137);
    }
}
