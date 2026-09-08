//! HiFi Wizard (DAC setup) — the Qt controller.
//!
//! Port of `crates/qbz/src/main.rs:21613-21845` (the Slint glue) + the state
//! half of `primitives/DacWizardModal.slint`. Every COMPUTATION lives in
//! `qbz-dac-wizard-core`, which the Slint adapter also uses — the wizard emits
//! PipeWire/WirePlumber snippets the user pastes into their own system, so a
//! second implementation of that logic is the one divergence this port must
//! never create (ADR-006; the crate split landed with this file).
//!
//! READ-ONLY, like the reference: this never writes a system file and never
//! runs a command. It probes, it formats text, and on the test step it plays
//! music through the normal queue path.
//!
//! ## What Rust owns and what QML owns
//!
//! Rust owns everything it COMPUTES (the probe verdict, the candidate list and
//! its checkboxes, the manual node name + its validity, the generated configs
//! and their accordion state, the read-back labels) plus `open` — because the
//! Settings row opens the wizard and the reset + probe must be one atomic act.
//!
//! QML owns the pure NAVIGATION state: `step`, the welcome checkbox, the three
//! review progress checkboxes and the manual-entry disclosure. That is exactly
//! the reference's split — `DacWizardModal.slint` mutates those inline and
//! never calls Rust for them (`:817 step -= 1`, `:853 step += 1`). They reset
//! on `openSeq`, the counter this file bumps on every open.
//!
//! ## One document, one publish
//!
//! `QbzDacWizard.wizardJson` carries the whole state (the port's settings
//! pattern). Rust mutates `STATE` and republishes; QML re-renders. No control
//! keeps its own copy of anything Rust owns.

use std::sync::Mutex;

use qbz_dac_wizard_core as core;
use qbz_models::Track;
use serde::Serialize;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One enumerated sink plus the user's checkbox. `checked` is Rust-owned so
/// `checked_dacs()` cannot disagree with what the user sees.
#[derive(Clone, Default, Serialize)]
struct Candidate {
    id: String,
    description: String,
    bus: String,
    #[serde(rename = "isDefault")]
    is_default: bool,
    #[serde(rename = "looksLikeDac")]
    looks_like_dac: bool,
    checked: bool,
    #[serde(rename = "ratesLabel")]
    rates_label: String,
}

/// One selected DAC's generated config + its accordion state.
#[derive(Clone, Default, Serialize)]
struct ConfigRow {
    name: String,
    #[serde(rename = "nodeName")]
    node_name: String,
    #[serde(rename = "pipewireConf")]
    pipewire_conf: String,
    #[serde(rename = "pulseConf")]
    pulse_conf: String,
    #[serde(rename = "wireplumberConf")]
    wireplumber_conf: String,
    expanded: bool,
}

#[derive(Clone, Default, Serialize)]
struct Remediation {
    caption: String,
    command: String,
}

#[derive(Clone, Default, Serialize)]
struct WizardDoc {
    open: bool,
    /// Bumped on every open — QML drops its navigation state when it changes
    /// (the `PlaylistImportModal.resetSeq` convention).
    #[serde(rename = "openSeq")]
    open_seq: u32,

    // check
    #[serde(rename = "distroOptions")]
    distro_options: Vec<String>,
    #[serde(rename = "distroIndex")]
    distro_index: i32,
    #[serde(rename = "initOptions")]
    init_options: Vec<String>,
    #[serde(rename = "initIndex")]
    init_index: i32,
    #[serde(rename = "healthOk")]
    health_ok: bool,
    #[serde(rename = "healthSummary")]
    health_summary: String,
    remediations: Vec<Remediation>,
    sandboxed: bool,
    #[serde(rename = "sandboxName")]
    sandbox_name: String,

    // select-dacs
    candidates: Vec<Candidate>,
    detecting: bool,
    #[serde(rename = "hasEnumeration")]
    has_enumeration: bool,
    #[serde(rename = "anyDacSelected")]
    any_dac_selected: bool,
    #[serde(rename = "manualNodeName")]
    manual_node_name: String,
    #[serde(rename = "manualValid")]
    manual_valid: bool,
    #[serde(rename = "manualDacType")]
    manual_dac_type: String,

    // test
    #[serde(rename = "testRequestedLabel")]
    test_requested_label: String,
    #[serde(rename = "testNegotiatedLabel")]
    test_negotiated_label: String,
    #[serde(rename = "testRateMatched")]
    test_rate_matched: bool,
    #[serde(rename = "testPlaying")]
    test_playing: bool,
    /// The four curated test tracks, PRE-FORMATTED here so QML never builds a
    /// label (the port's rule; also keeps the strings in one catalog).
    #[serde(rename = "testTracks")]
    test_tracks: Vec<String>,

    // review + done
    #[serde(rename = "dacConfigs")]
    dac_configs: Vec<ConfigRow>,
    #[serde(rename = "backupCmd")]
    backup_cmd: String,
    #[serde(rename = "restartCmd")]
    restart_cmd: String,
    #[serde(rename = "createdPaths")]
    created_paths: Vec<String>,
}

/// Everything the wizard owns, including the pieces that are NOT in the
/// document: the cached probe (so a distro override recomputes without
/// re-shelling) and the resolved test tracks (so the user can jump between
/// them).
#[derive(Default)]
struct State {
    doc: WizardDoc,
    last_health: Option<qbz_audio::AudioStackHealth>,
    test_tracks: Vec<Track>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

/// Serialize + hand the document to the Qt thread.
fn publish() {
    let json = with_state(|s| serde_json::to_string(&s.doc).unwrap_or_else(|_| "{}".to_string()));
    crate::dac_wizard_bridge::ui(move |mut b| {
        b.as_mut().set_wizard_json(cxx_qt_lib::QString::from(&json));
    });
}

/// The document as QML first sees it — closed, and parseable so a binding that
/// reads `doc.open` on frame 1 cannot throw.
pub fn initial_json() -> String {
    serde_json::to_string(&WizardDoc {
        has_enumeration: true,
        ..Default::default()
    })
    .unwrap_or_else(|_| "{\"open\":false}".to_string())
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

/// Settings > Audio > "Open Wizard". Resets the whole wizard, seeds the two
/// dropdowns from auto-detection, shows the "checking…" placeholder, then runs
/// the (shelling) health probe off the UI thread.
///
/// 1:1 with `dac_wizard::open_immediate` + the `spawn_blocking` probe that
/// follows it in the reference's `on_open`.
pub fn open() {
    with_state(|s| {
        let seq = s.doc.open_seq.wrapping_add(1);
        let sandbox = core::sandbox_name();
        s.last_health = None;
        s.test_tracks.clear();
        s.doc = WizardDoc {
            open: true,
            open_seq: seq,
            distro_options: core::distro_options(),
            distro_index: core::detected_distro_index(),
            init_options: core::init_options(),
            init_index: core::detected_init_index(),
            sandboxed: !sandbox.is_empty(),
            sandbox_name: sandbox.to_string(),
            health_summary: qbz_i18n::t("Checking your audio stack…"),
            has_enumeration: true,
            test_tracks: vec![
                qbz_i18n::t("16-bit / 44.1 kHz — George Harrison · My Sweet Lord"),
                qbz_i18n::t("24-bit / 44.1 kHz — Billie Eilish · LUNCH"),
                qbz_i18n::t("24-bit / 96 kHz — Iron Maiden · Stratego"),
                qbz_i18n::t("24-bit / 192 kHz — Toto · Africa"),
            ],
            ..Default::default()
        };
    });
    publish();
    log::info!("[qbz-qt] dac wizard opened (probing the audio stack)");

    // The probe shells out (systemctl, aplay, pw-dump); never on the UI thread.
    crate::spawn(async move {
        let health = tokio::task::spawn_blocking(core::probe_health)
            .await
            .unwrap_or_else(|e| {
                log::warn!("[qbz-qt] dac wizard: health probe panicked: {e}");
                qbz_audio::AudioStackHealth {
                    wireplumber_active: false,
                    has_pw_dump: false,
                    cpal_sees_pipewire: false,
                    has_pactl: false,
                    any_devices: false,
                }
            });
        with_state(|s| s.last_health = Some(health));
        recompute_check();
        let (ok, n) = with_state(|s| (s.doc.health_ok, s.doc.remediations.len()));
        log::info!("[qbz-qt] dac wizard health: ok={ok} remediations={n}");
    });
}

/// Close. Leaves the document otherwise intact — reopening resets it anyway,
/// and a close mid-test must not silently stop the music (the reference's
/// scrim/X close nothing but the overlay).
pub fn close() {
    with_state(|s| s.doc.open = false);
    publish();
}

// ---------------------------------------------------------------------------
// Check step
// ---------------------------------------------------------------------------

/// User overrode the distro (package manager) — recompute the commands.
pub fn set_distro(index: i32) {
    with_state(|s| s.doc.distro_index = index);
    recompute_check();
}

/// User overrode the init system (service commands) — recompute.
pub fn set_init(index: i32) {
    with_state(|s| s.doc.init_index = index);
    recompute_check();
    // The review step's step-3 command is init-aware too; keep it honest if the
    // user goes back and changes the init AFTER generating the configs.
    let restart = with_state(|s| core::restart_command_for(s.doc.init_index));
    let changed = with_state(|s| {
        if s.doc.restart_cmd.is_empty() || s.doc.restart_cmd == restart {
            false
        } else {
            s.doc.restart_cmd = restart;
            true
        }
    });
    if changed {
        publish();
    }
}

/// Rebuild the check step from the cached probe + the current dropdowns. Uses
/// the cache so a dropdown change never re-shells (reference `recompute`).
fn recompute_check() {
    with_state(|s| {
        let health = s.last_health.unwrap_or_else(core::probe_health);
        let view = core::check_view(
            health,
            s.doc.distro_index,
            s.doc.init_index,
            s.doc.sandboxed,
        );
        s.doc.health_ok = view.health_ok;
        s.doc.health_summary = view.summary;
        s.doc.remediations = view
            .remediations
            .into_iter()
            .map(|r| Remediation {
                caption: r.caption,
                command: r.command,
            })
            .collect();
    });
    publish();
}

// ---------------------------------------------------------------------------
// Select-DACs step
// ---------------------------------------------------------------------------

/// Entering the DACs step: show "detecting…", then enumerate off the UI thread.
/// Pre-selects the likely DACs; an empty enumeration flips `hasEnumeration` off
/// so the manual escape hatch opens unconditionally.
pub fn run_detect() {
    with_state(|s| s.doc.detecting = true);
    publish();

    crate::spawn(async move {
        let data = tokio::task::spawn_blocking(core::detect_blocking)
            .await
            .unwrap_or_else(|e| {
                log::warn!("[qbz-qt] dac wizard: enumeration panicked: {e}");
                Vec::new()
            });
        with_state(|s| {
            let rows: Vec<Candidate> = data
                .iter()
                .map(|d| Candidate {
                    id: d.id.clone(),
                    description: d.description.clone(),
                    bus: d.bus.clone(),
                    is_default: d.is_default,
                    looks_like_dac: d.looks_like_dac,
                    checked: d.looks_like_dac,
                    rates_label: d.rates_label.clone(),
                })
                .collect();
            s.doc.any_dac_selected = rows.iter().any(|r| r.checked);
            s.doc.has_enumeration = !data.is_empty();
            s.doc.candidates = rows;
            s.doc.detecting = false;
        });
        publish();
    });
}

/// Flip one candidate's checkbox + recompute the Next gate.
pub fn toggle_dac(index: i32) {
    with_state(|s| {
        if let Some(row) = s.doc.candidates.get_mut(index.max(0) as usize) {
            row.checked = !row.checked;
        }
        s.doc.any_dac_selected = s.doc.candidates.iter().any(|r| r.checked);
    });
    publish();
}

/// Validate a manually-pasted `node.name` (escape hatch).
///
/// The typed text is stored here because `checked_dacs()` reads it, but QML
/// keeps its own field text and never re-reads this back — republishing into a
/// focused TextInput is how a cursor ends up jumping to the end mid-word.
pub fn validate_manual(text: &str) {
    with_state(|s| {
        s.doc.manual_node_name = text.to_string();
        s.doc.manual_valid = core::validate_node_name(text);
        s.doc.manual_dac_type = core::detect_dac_type(text).to_string();
    });
    publish();
}

/// (node_name, display_name) for every checked candidate, or a valid manual one.
fn checked_dacs() -> Vec<(String, String)> {
    with_state(|s| {
        let mut out: Vec<(String, String)> = s
            .doc
            .candidates
            .iter()
            .filter(|c| c.checked)
            .map(|c| (c.id.clone(), c.description.clone()))
            .collect();
        if out.is_empty() {
            let manual = s.doc.manual_node_name.trim().to_string();
            if !manual.is_empty() && s.doc.manual_valid {
                out.push((manual.clone(), manual));
            }
        }
        out
    })
}

// ---------------------------------------------------------------------------
// Review step
// ---------------------------------------------------------------------------

/// Entering the review step: re-probe rates and build the three snippets per
/// selected DAC, off the UI thread. One DAC → expanded; several → collapsed
/// accordions (reference `apply_configs`).
pub fn gen_configs() {
    let dacs = checked_dacs();
    crate::spawn(async move {
        let data = tokio::task::spawn_blocking(move || core::gen_configs_blocking(dacs))
            .await
            .unwrap_or_else(|e| {
                log::warn!("[qbz-qt] dac wizard: config generation panicked: {e}");
                Vec::new()
            });
        with_state(|s| {
            let single = data.len() == 1;
            s.doc.created_paths = core::created_paths(&data);
            s.doc.dac_configs = data
                .into_iter()
                .map(|d| ConfigRow {
                    name: d.name,
                    node_name: d.node_name,
                    pipewire_conf: d.pipewire_conf,
                    pulse_conf: d.pulse_conf,
                    wireplumber_conf: d.wireplumber_conf,
                    expanded: single,
                })
                .collect();
            s.doc.backup_cmd = core::BACKUP_CMD.to_string();
            s.doc.restart_cmd = core::restart_command_for(s.doc.init_index);
        });
        publish();
    });
}

/// Collapse/expand one DAC's generated-config accordion.
pub fn toggle_config(index: i32) {
    with_state(|s| {
        if let Some(row) = s.doc.dac_configs.get_mut(index.max(0) as usize) {
            row.expanded = !row.expanded;
        }
    });
    publish();
}

// ---------------------------------------------------------------------------
// Test step
// ---------------------------------------------------------------------------

/// Show the "playing" state. The read-back probes whichever DAC is actively
/// playing (scan), so no node needs to be stashed.
fn begin_test(s: &mut State) {
    s.doc.test_playing = true;
    s.doc.test_rate_matched = false;
    s.doc.test_requested_label = qbz_i18n::t("Starting…");
    s.doc.test_negotiated_label = String::new();
}

fn reject_test_control(action: &str) {
    log::info!("[qbz-qt] dac wizard: {action} refused while QConnect owns playback");
    crate::toast_qt::info(qbz_i18n::t(
        "QConnect is controlling playback. Return playback to QBZ before using the DAC test.",
    ));
}

/// "Play test": resolve the four curated tracks (id-hint first, then an
/// "artist title" search when the id 404s on a pulled licence — never
/// raw-id-only), stash them so the user can jump between them, and play.
pub fn start_test() {
    with_state(begin_test);
    publish();

    crate::spawn(async move {
        let runtime = crate::app();
        let mut tracks: Vec<Track> = Vec::new();
        for seed in core::TEST_SEEDS.iter() {
            let mut chosen = match runtime.core().get_track(seed.id_hint).await {
                Ok(t) if core::track_matches_seed(&t, seed) => Some(t),
                _ => None,
            };
            if chosen.is_none() {
                let q = format!("{} {}", seed.artist, seed.title);
                if let Ok(page) = runtime.core().search_tracks(&q, 10, 0, None).await {
                    chosen = page
                        .items
                        .into_iter()
                        .find(|t| core::track_matches_seed(t, seed));
                }
            }
            if let Some(t) = chosen {
                tracks.push(t);
            }
        }

        if tracks.is_empty() {
            with_state(|s| {
                s.doc.test_playing = false;
                s.doc.test_requested_label =
                    qbz_i18n::t("Couldn't load the test tracks (offline?)");
                s.doc.test_negotiated_label = String::new();
                s.doc.test_rate_matched = false;
            });
            publish();
            return;
        }
        with_state(|s| s.test_tracks = tracks.clone());
        play_from(tracks, 0).await;
    });
}

/// Jump straight to one of the four tracks (skip the long waits) by re-setting
/// the queue at that index through the working play path.
pub fn test_play_index(index: i32) {
    crate::spawn(async move {
        let tracks = with_state(|s| s.test_tracks.clone());
        if tracks.is_empty() {
            return;
        }
        let start = (index.max(0) as usize).min(tracks.len() - 1);
        play_from(tracks, start).await;
    });
}

/// Build the queue from catalog tracks and start at `start`. A FLAT list, so no
/// container context is stamped — 1:1 with the reference's
/// `play_tracks_ctx(.., None)`.
async fn play_from(tracks: Vec<Track>, start: usize) {
    let runtime = crate::app();
    let queue: Vec<qbz_models::QueueTrack> = tracks
        .iter()
        .map(crate::foryou_qt::to_queue_track)
        .collect();
    if let Err(e) = crate::playback_qt::play_track_list(&runtime, queue, start, false).await {
        log::warn!("[qbz-qt] dac wizard: test playback failed: {e}");
    }
}

/// Stop the test. Pauses (the reference does exactly this — it does not clear
/// the queue, so the user keeps whatever was loaded).
pub fn stop_test() {
    crate::spawn(async move {
        let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
            reject_test_control("stop test");
            return;
        };
        let _ = crate::app().core().pause();
        with_state(|s| s.doc.test_playing = false);
        publish();
    });
}

/// "Use my current queue": start the read-back on the user's own music instead
/// of the curated tracks. Guardrail — an empty queue gets a hint rather than a
/// read-back that would sit forever on "Nothing playing".
pub fn verify_own() {
    crate::spawn(async move {
        // Keep the permit over the queue read and the eventual resume: a
        // handoff must either drain this whole action first or make it a clean
        // refusal before any delegated queue/player state is observed.
        let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
            reject_test_control("use current queue");
            return;
        };
        let runtime = crate::app();
        let (tracks, _) = runtime.core().get_all_queue_tracks().await;
        if tracks.is_empty() {
            with_state(|s| {
                s.doc.test_playing = false;
                s.doc.test_rate_matched = false;
                s.doc.test_negotiated_label = String::new();
                s.doc.test_requested_label =
                    qbz_i18n::t("Your queue is empty — add some tracks first, or press Play test.");
            });
            publish();
            return;
        }
        let _ = runtime.core().resume();
        with_state(begin_test);
        publish();
    });
}

/// One poll of the read-back: what QBZ asked the device for vs what the DAC is
/// really clocked at (N6). Driven by a QML `Timer` at 1.5 s while the test
/// plays, exactly as the reference drives it.
pub fn poll_test() {
    crate::spawn(async move {
        let runtime = crate::app();
        let player = runtime.core().player();
        let req_rate = player.state.get_sample_rate();
        let req_bits = player.state.get_bit_depth();
        // The ALSA probe reads /proc; keep it off the UI thread like the rest.
        let negotiated = tokio::task::spawn_blocking(qbz_audio::negotiated_active_rate)
            .await
            .unwrap_or(None);
        let view = core::poll_view(req_rate, req_bits, negotiated);
        with_state(|s| {
            s.doc.test_requested_label = view.requested_label;
            s.doc.test_negotiated_label = view.negotiated_label;
            s.doc.test_rate_matched = view.rate_matched;
        });
        publish();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `STATE` is a process-global and cargo runs these in PARALLEL threads,
    /// so three tests that each seed and clear it raced and failed each other
    /// (caught by the gate, 2026-08-11). Every test that touches the wizard
    /// state takes this lock for its whole body.
    ///
    /// `lock()` rather than `lock().unwrap()`: a test that panics while
    /// holding it would poison the mutex and turn one real failure into three,
    /// hiding which one is the actual defect.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *STATE.lock().unwrap() = None;
        g
    }

    /// The pre-publish document must parse in QML AND must not put the manual
    /// escape hatch on screen before enumeration has ever run (`hasEnumeration`
    /// false is what forces it open).
    #[test]
    fn initial_document_is_closed_and_parseable() {
        let v: serde_json::Value = serde_json::from_str(&initial_json()).unwrap();
        assert_eq!(v["open"], serde_json::json!(false));
        assert_eq!(v["hasEnumeration"], serde_json::json!(true));
    }

    /// `checked_dacs` prefers the CHECKED candidates and only falls back to the
    /// manual field when nothing is checked — and never accepts an invalid one.
    #[test]
    fn checked_dacs_prefers_candidates_then_valid_manual() {
        let _g = guard();
        with_state(|s| {
            s.doc.candidates = vec![
                Candidate {
                    id: "alsa_output.usb-a".into(),
                    description: "DacMagic".into(),
                    checked: true,
                    ..Default::default()
                },
                Candidate {
                    id: "alsa_output.pci-b".into(),
                    description: "Onboard".into(),
                    checked: false,
                    ..Default::default()
                },
            ];
            s.doc.manual_node_name = "alsa_output.usb-manual".into();
            s.doc.manual_valid = true;
        });
        let picked = checked_dacs();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].0, "alsa_output.usb-a");

        // Nothing checked -> the valid manual entry.
        with_state(|s| s.doc.candidates.clear());
        let picked = checked_dacs();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].0, "alsa_output.usb-manual");

        // An INVALID manual entry is not a DAC.
        with_state(|s| s.doc.manual_valid = false);
        assert!(checked_dacs().is_empty());
    }

    /// Toggling a candidate must move the Next gate with it — the gate is what
    /// stops the user reaching a review step with nothing to generate.
    #[test]
    fn toggling_a_candidate_moves_the_next_gate() {
        let _g = guard();
        with_state(|s| {
            s.doc.candidates = vec![Candidate {
                id: "alsa_output.usb-a".into(),
                checked: false,
                ..Default::default()
            }];
            s.doc.any_dac_selected = false;
        });
        toggle_dac(0);
        assert!(with_state(|s| s.doc.any_dac_selected));
        toggle_dac(0);
        assert!(!with_state(|s| s.doc.any_dac_selected));
    }

    /// An out-of-range index from QML must not panic (a recycled delegate can
    /// fire one after the model shrank).
    #[test]
    fn out_of_range_toggles_are_ignored() {
        let _g = guard();
        toggle_dac(7);
        toggle_config(-3);
        assert!(with_state(|s| s.doc.candidates.is_empty()));
    }
}
