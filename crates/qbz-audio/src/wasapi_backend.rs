//! WASAPI endpoint capabilities: what rates a device accepts in EXCLUSIVE
//! mode, and being told when the device list changes.
//!
//! The Windows counterpart of `alsa_backend::get_device_supported_rates`. It
//! exists because "what can this DAC actually do" cannot be inferred: the
//! spike measured a Cambridge Audio USB DAC accepting 44100, 48000, 88200,
//! 96000 and 192000 while REFUSING 176400 -- the 44.1 family stops at 88.2
//! while the 48 family reaches 192, which no rule about "higher rates imply
//! lower ones" predicts. The owner confirmed it from the hardware, and a
//! SECOND endpoint on the same machine does accept 176400, so the difference
//! is the device rather than the driver stack.
//!
//! ## Exclusive mode is the question, and it is a different question
//!
//! `IsFormatSupported(SHARED)` answers about the mixer, which resamples and
//! therefore says yes to almost everything. Only the EXCLUSIVE answer says
//! what reaches the converter untouched, which is the only answer bit-perfect
//! playback cares about.
//!
//! ## "I could not ask" is not "it supports nothing"
//!
//! A busy endpoint, a failed activation and a device that genuinely accepts no
//! rate all produce an empty list, and collapsing them was a real defect in the
//! first draft: the empty answer was cached as authoritative, and because a
//! stream CLOSING raises no device notification, it would have outlived the
//! condition that caused it. [`Caps`] keeps them apart, and only `Known` is
//! ever stored.

#![cfg(windows)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::wasapi_direct::LADDER;

/// The rates worth asking about: the two families QBZ streams and their
/// multiples, plus the DSD-over-PCM carriers. 352800 and 384000 are asked
/// because a DAC that takes them changes what a quality cap can offer.
const SWEEP: [u32; 8] = [
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
];

/// What a probe learned, which is not always a capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caps {
    /// The device answered. An empty vector HERE is a real "nothing".
    Known(Vec<u32>),
    /// The device could not be asked -- busy, gone, or COM refused. Never
    /// cached: nothing about it is a property of the hardware.
    Unavailable,
}

/// Bumped by every device notification. A cached answer is only good while
/// this has not moved.
///
/// A COUNTER rather than per-endpoint removal, and that is the point: the
/// notifications arrive on a thread the Windows audio system owns, and
/// Microsoft's rule there is DO NOT BLOCK. `fetch_add` cannot block, cannot
/// fail and cannot lose an invalidation, where taking the cache's mutex would
/// block a thread that was told not to, and a `try_lock` that skipped on
/// contention would silently keep a stale answer.
///
/// Invalidating EVERYTHING on any event is also the honest granularity: a
/// device arriving or leaving changes exclusive-mode contention for endpoints
/// other than itself, so "only that one changed" was never true.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Endpoint id -> (the generation it was measured in, the rates it accepted).
fn cache() -> &'static Mutex<HashMap<String, (u64, Vec<u32>)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (u64, Vec<u32>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Serialises the probing itself, so two callers that miss together do the
/// work once instead of twice.
///
/// MEASURED, not theoretical: startup enumerates from two independent places
/// -- the device-capability pass and the DAC-capability pass -- and the log
/// showed every endpoint swept exactly twice, both racing past the empty
/// cache. Holding this across the sweep costs the second caller about a
/// millisecond per endpoint and saves it the same again.
///
/// SEPARATE from the cache mutex on purpose. The cache lock is taken for a
/// `get` and an `insert` and released immediately; this one is held across COM
/// calls, and merging them would make every reader wait behind a probe. The
/// hotplug callbacks touch NEITHER -- they only bump an atomic -- so nothing
/// the audio system owns can ever wait on either lock.
fn sweep_gate() -> &'static Mutex<()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
}

/// The cache key: the endpoint, and the QUESTION the stored answer answers.
///
/// The value is "some rung of the ladder accepts this rate", so naming one
/// rung would claim a precision it does not have. `LADDER.len()` is part of it
/// because growing the ladder changes the answer for the same device, and an
/// entry written under a shorter ladder must not survive that.
pub(crate) fn rate_sweep_key(endpoint_id: &str) -> String {
    format!("{endpoint_id}|any-of-{}-rungs", LADDER.len())
}

/// COM is initialised at most ONCE per thread, and deliberately never undone.
///
/// `initialize_mta` should be balanced by a `CoUninitialize`, but the threads
/// that reach here are pool workers that get reused: uninitialising on the way
/// out would tear the apartment down under whatever COM work the same thread
/// runs next, and doing it per sweep would re-enter the apartment on every
/// call. One initialisation per worker for the life of the process is the
/// bounded cost, and it is the trade any COM-using code on a pool makes.
///
/// `RPC_E_CHANGED_MODE` -- this thread is already an STA -- is not an error to
/// report: `IsFormatSupported` is happy either way.
fn ensure_com() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    DONE.with(|d| {
        if !d.get() {
            let _ = wasapi::initialize_mta().ok();
            d.set(true);
        }
    });
}

/// The rates `endpoint_id` accepts in EXCLUSIVE mode.
///
/// `None` = the endpoint could not be asked. Callers must NOT read that as
/// "supports nothing" -- see [`Caps`] -- and in particular must not use it to
/// justify sending a device a format it has not agreed to.
pub fn supported_rates(endpoint_id: &str) -> Option<Vec<u32>> {
    let key = rate_sweep_key(endpoint_id);
    // Read BEFORE probing, and store against THIS value rather than the
    // current one. If a device event lands while the sweep runs, the result is
    // already stale and the next caller re-measures instead of trusting it.
    let generation = GENERATION.load(Ordering::SeqCst);

    // The lock is never held across the sweep below: that is milliseconds of
    // COM calls, and nothing else may be made to wait on it.
    if let Ok(guard) = cache().lock() {
        if let Some((gen, hit)) = guard.get(&key) {
            if *gen == generation {
                return Some(hit.clone());
            }
        }
    }

    // Take the gate, then look again: the caller that held it may have been
    // measuring this very endpoint, in which case its answer is now in the
    // cache and there is nothing left to do.
    let _gate = sweep_gate().lock();
    if let Ok(guard) = cache().lock() {
        if let Some((gen, hit)) = guard.get(&key) {
            if *gen == generation {
                return Some(hit.clone());
            }
        }
    }

    match sweep(endpoint_id) {
        Caps::Known(rates) => {
            if let Ok(mut guard) = cache().lock() {
                guard.insert(key, (generation, rates.clone()));
            }
            Some(rates)
        }
        // NOT cached. A busy or absent endpoint says nothing about the
        // hardware, and a stream closing raises no notification to clear it.
        Caps::Unavailable => None,
    }
}

/// The highest rate the endpoint accepts, or `None` if it could not be asked
/// or accepted nothing.
pub fn max_supported_rate(endpoint_id: &str) -> Option<u32> {
    supported_rates(endpoint_id)?.into_iter().max()
}

/// Drop everything we believe about every endpoint.
///
/// Lock-free by construction, because the hotplug callbacks call it from a
/// thread the audio system owns and are required not to block there.
pub fn invalidate_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Ask the device about every rate in `SWEEP`, at every rung of the format
/// ladder, and keep a rate the moment ANY rung accepts it.
///
/// Walking the ladder rather than one rung matters: the spike's DAC accepts
/// ONLY 24-in-32 and refuses packed 24, 32-bit, 16-bit and float, so a sweep
/// pinned to a single rung would have reported that it supports nothing.
fn sweep(endpoint_id: &str) -> Caps {
    use wasapi::{DeviceEnumerator, SampleType, ShareMode, WaveFormat};

    ensure_com();

    let Ok(enumerator) = DeviceEnumerator::new() else {
        log::warn!("[wasapi-caps] no device enumerator; capabilities unknown");
        return Caps::Unavailable;
    };
    let Ok(device) = enumerator.get_device(endpoint_id) else {
        log::warn!("[wasapi-caps] endpoint {endpoint_id} unavailable; capabilities unknown");
        return Caps::Unavailable;
    };
    // ONE client for every question. `IsFormatSupported` may be called at any
    // time after activation and neither initialises nor reserves the endpoint,
    // so a client per probe was forty activations buying nothing. Only an
    // INITIALISED exclusive client reserves a device, and this one never is.
    let Ok(client) = device.get_iaudioclient() else {
        log::warn!("[wasapi-caps] cannot activate {endpoint_id}; capabilities unknown");
        return Caps::Unavailable;
    };

    let mut rates = Vec::new();
    for rate in SWEEP {
        let accepted = LADDER.iter().any(|rung| {
            // The same shape `wasapi_direct`'s open path builds, and it must
            // stay the same shape: a rung probed one way and opened another is
            // a device that says yes and then refuses.
            let ty = if rung.is_float() {
                SampleType::Float
            } else {
                SampleType::Int
            };
            let format = WaveFormat::new(
                rung.container_bits() as usize,
                rung.valid_bits() as usize,
                &ty,
                rate as usize,
                2,
                None,
            );
            // `is_supported`, NOT `is_supported_exclusive_with_quirks`. That
            // helper retries as a plain WAVEFORMATEX and with alternative
            // channel masks, and using it HERE ALONE would be a mistake: this
            // sweep must predict what the OPEN path does, and that path asks
            // the plain question. A rate accepted only through a quirk the
            // opener never tries is a rate that appears in the UI and then
            // fails to open -- worse than under-reporting.
            //
            // Teaching the opener those quirks is worth doing for DACs that
            // need them, and it has to happen in BOTH places in one change, or
            // the two disagree again in the other direction.
            //
            // In exclusive mode `ppClosestMatch` is null, so the answer is an
            // exact yes or an error: there is no S_FALSE "closest match" to
            // weigh, which is shared-mode behaviour.
            client.is_supported(&format, &ShareMode::Exclusive).is_ok()
        });
        if accepted {
            rates.push(rate);
        }
    }

    log::info!("[wasapi-caps] {endpoint_id} accepts {rates:?} in exclusive mode");
    Caps::Known(rates)
}

// ---------------------------------------------------------------------------
// Hotplug
// ---------------------------------------------------------------------------

/// Whether the watch thread is running. The REGISTRATION itself cannot live in
/// a static: it holds COM interface pointers and is therefore not `Send`.
static WATCH_STARTED: AtomicBool = AtomicBool::new(false);

/// Start listening for endpoints arriving, leaving or changing state.
///
/// Idempotent, and it RE-ARMS: if the audio service is unavailable when this
/// first runs, the flag is cleared so a later call can retry. A transient
/// failure must not disable the watch for the life of the process, which the
/// first draft did by resetting only when the thread failed to spawn.
///
/// SCOPE, stated plainly: this keeps the CAPABILITY CACHE honest. It does not
/// refresh an already-rendered device list -- a newly attached DAC appears
/// when something enumerates again, not the instant it is plugged in. Making
/// the settings model live needs a bridge signal and belongs with that model.
pub fn start_hotplug_watch() {
    if WATCH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    // A DEDICATED THREAD that owns the registration and then parks forever.
    //
    // Not a matter of style: `DeviceEventRegistration` holds COM interface
    // pointers, so it is not `Send` and cannot be stored in a static. Keeping
    // it on the stack of a thread that never returns is what keeps it alive,
    // and dropping it is exactly what unregisters the callbacks -- so the
    // thread outliving everything IS the lifetime management. It also gives
    // the registration its own apartment rather than borrowing whichever
    // thread happened to call first.
    let spawned = std::thread::Builder::new()
        .name("qbz-wasapi-hotplug".to_string())
        .spawn(|| {
            use wasapi::{DeviceEnumerator, DeviceEventCallbacks};

            ensure_com();
            let Ok(enumerator) = DeviceEnumerator::new() else {
                log::warn!("[wasapi-caps] hotplug watch unavailable; will retry on next request");
                WATCH_STARTED.store(false, Ordering::SeqCst);
                return;
            };

            // EVERY callback below runs on a thread the Windows audio system
            // owns, where the documented rule is that they return IMMEDIATELY.
            // So each does one lock-free atomic increment and NOTHING else --
            // no mutex, and no logging either: a log macro can allocate, take
            // its own lock and perform synchronous output, and "it is only a
            // log line" is exactly how that rule gets broken.
            let mut callbacks = DeviceEventCallbacks::new();
            callbacks.set_device_removed_callback(|_id| invalidate_all());
            callbacks.set_device_added_callback(|_id| invalidate_all());
            callbacks.set_device_state_callback(|_id, _state| invalidate_all());
            // Reconfiguring a device in Windows' own Sound settings changes
            // what it accepts without adding or removing anything. The first
            // draft's comment promised this and installed nothing.
            callbacks.set_property_value_callback(|_id, _key| invalidate_all());

            let _registration = match enumerator.register_notification_callback(callbacks) {
                Ok(reg) => reg,
                Err(e) => {
                    log::warn!("[wasapi-caps] could not register the hotplug watch: {e}");
                    WATCH_STARTED.store(false, Ordering::SeqCst);
                    return;
                }
            };
            log::info!("[wasapi-caps] hotplug watch registered");

            // Park. `_registration` must not be dropped, and there is nothing
            // for this thread to do: notifications arrive on WASAPI's own
            // thread, not this one.
            loop {
                std::thread::park();
            }
        });
    if let Err(e) = spawned {
        WATCH_STARTED.store(false, Ordering::SeqCst);
        log::warn!("[wasapi-caps] could not spawn the hotplug watch thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_key_says_what_the_answer_means() {
        // The stored value is "some rung accepts this rate", so the key must
        // not name one rung -- and it must change if the ladder grows, or an
        // entry written under a shorter ladder would outlive its question.
        let key = rate_sweep_key("{0.0.0.x}");
        assert!(key.starts_with("{0.0.0.x}|"));
        assert!(key.contains(&LADDER.len().to_string()));
        assert_ne!(rate_sweep_key("{0.0.0.x}"), rate_sweep_key("{0.0.0.y}"));
    }

    #[test]
    fn unavailable_is_not_an_empty_capability() {
        // The distinction the first draft collapsed: a busy endpoint and one
        // that genuinely accepts nothing are different answers, and only the
        // second is a fact about the hardware.
        assert_ne!(Caps::Unavailable, Caps::Known(Vec::new()));
        assert_eq!(Caps::Known(vec![44_100]), Caps::Known(vec![44_100]));
    }

    #[test]
    fn a_device_event_expires_every_cached_answer() {
        let before = GENERATION.load(Ordering::SeqCst);
        invalidate_all();
        assert_ne!(GENERATION.load(Ordering::SeqCst), before);
    }
}
