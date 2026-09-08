//! Shared cleanup for raw CPAL output-device enumeration.
//!
//! CPAL's Linux/ALSA host exposes *every* libasound PCM: the `null` sink
//! ("Discard all samples …"), the bare routing/format plugins, and one
//! entry per card × profile. Worse, several distinct PCM ids collapse to
//! the **same** human description — so a raw `host.output_devices()` dump
//! shows "Discard all samples", 8× "HDA Intel PCH, ALC3254 Analog", N×
//! "Cambridge Audio USB Audio 2.0", etc. That is useless as a picker.
//!
//! The precise pickers (the ALSA + PipeWire backends) build their lists
//! from `/proc/asound` and `pactl`, so they already produce one entry per
//! real output. The CPAL-default path (`CpalDefaultBackend` = "System",
//! and the JACK placeholder) and the `output_sinks` diagnostic had **no**
//! filtering at all — this module closes that gap with the same intent
//! the ALSA backend already encodes: *deduplicated entries, real outputs
//! first.* The `null` discard sink is kept (it is a legitimate "send to
//! nowhere" target) but always sorted to the **end** of the list, never
//! offered as a first-class output.
//!
//! Host-agnostic on purpose: PipeWire node names (`alsa_output.*`) and
//! macOS/Windows device names carry unique displays, so they pass through
//! untouched (the helper is a no-op there beyond dropping a stray discard
//! sink). Pure string logic — unit-tested without any audio host.

/// True for the ALSA `null` PCM, whose CPAL description is
/// "Discard all samples (playback) or generate zero samples (capture)".
/// It never reaches hardware, so it must never appear in an output picker.
pub fn is_discard_sink(display: &str) -> bool {
    display
        .trim()
        .to_ascii_lowercase()
        .starts_with("discard all samples")
}

/// Dedup grain: distinct real outputs carry distinct human descriptions
/// (the ALSA host names analog "… Analog", S/PDIF "… Digital", "HDMI 0/1",
/// each USB DAC by its product string), while the plugin-wrapper flavors
/// of one output (`front:`/`hw:`/`plughw:`/`surround*:`/`plug:` over the
/// same card+device) all share one description. Folding on the normalized
/// description collapses the wrappers and keeps the genuinely distinct
/// outputs.
fn dedup_key(display: &str) -> String {
    display
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Preference within a display group (lower = kept). When several PCM ids
/// share a description we keep the cleanest, most-openable id: the system
/// default and PipeWire/Pulse server sinks first, then PipeWire nodes, then
/// the `front:`/`sysdefault:` card aliases, leaving the `surround*`/`plug:`/
/// raw-`hw:` wrappers as last resort.
fn id_rank(id: &str) -> u8 {
    match id {
        "default" | "pipewire" | "pulse" | "sysdefault" => return 0,
        _ => {}
    }
    if id.starts_with("alsa_output.") {
        1
    } else if id.starts_with("front:CARD=") {
        2
    } else if id.starts_with("sysdefault:CARD=") {
        3
    } else if id.starts_with("iec958:CARD=") || id.starts_with("hdmi:CARD=") {
        4
    } else if id.starts_with("hw:") || id.starts_with("plughw:") {
        6
    } else if id.starts_with("surround")
        || id.starts_with("plug:")
        || id.starts_with("dmix")
        || id.starts_with("dsnoop")
        || id.starts_with("route")
    {
        9
    } else {
        5
    }
}

/// Collapse entries that share a display name (keeping the best-ranked id of
/// each group), drop blank rows, and emit **real outputs first** in first-seen
/// order with the `null` discard sink(s) pushed to the end.
///
/// Generic over the caller's row type so both the `AudioDevice` enumeration
/// and the `OutputSinkInfo` diagnostic reuse one tested implementation:
/// `id_of` yields the re-openable device id, `display_of` the shown name.
pub fn retain_real_outputs<T>(
    items: Vec<T>,
    id_of: impl Fn(&T) -> &str,
    display_of: impl Fn(&T) -> &str,
) -> Vec<T> {
    use std::collections::HashMap;

    // First pass: pick the winning index per display group, in first-seen order.
    // Discard sinks are tracked separately so they can be appended last.
    let mut winner: HashMap<String, usize> = HashMap::new();
    let mut real_order: Vec<String> = Vec::new();
    let mut discard_order: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let display = display_of(item);
        if display.trim().is_empty() {
            continue;
        }
        let key = dedup_key(display);
        match winner.get(&key).copied() {
            None => {
                winner.insert(key.clone(), i);
                if is_discard_sink(display) {
                    discard_order.push(key);
                } else {
                    real_order.push(key);
                }
            }
            Some(cur) => {
                if id_rank(id_of(item)) < id_rank(id_of(&items[cur])) {
                    winner.insert(key, i);
                }
            }
        }
    }

    // Second pass: emit real outputs first (first-seen order), then discard.
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(real_order.len() + discard_order.len());
    for key in real_order.into_iter().chain(discard_order) {
        if let Some(item) = slots[winner[&key]].take() {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        let owned: Vec<(String, String)> = rows
            .iter()
            .map(|(id, d)| (id.to_string(), d.to_string()))
            .collect();
        retain_real_outputs(owned, |r| r.0.as_str(), |r| r.1.as_str())
    }

    #[test]
    fn discard_sink_sorted_to_end() {
        assert!(is_discard_sink(
            "Discard all samples (playback) or generate zero samples (capture)"
        ));
        // null appears FIRST in the raw list but must be emitted last.
        let out = run(&[
            (
                "null",
                "Discard all samples (playback) or generate zero samples (capture)",
            ),
            (
                "default",
                "Default ALSA Output (currently PipeWire Media Server)",
            ),
            ("front:CARD=PCH,DEV=0", "HDA Intel PCH, ALC3254 Analog"),
        ]);
        let ids: Vec<&str> = out.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(ids, vec!["default", "front:CARD=PCH,DEV=0", "null"]);
    }

    #[test]
    fn collapses_plugin_wrappers_to_one_per_output() {
        // The exact shape of the user's listota: one analog output exposed
        // via many plugin ids that all share a description.
        let out = run(&[
            ("front:CARD=PCH,DEV=0", "HDA Intel PCH, ALC3254 Analog"),
            ("surround51:CARD=PCH,DEV=0", "HDA Intel PCH, ALC3254 Analog"),
            ("hw:CARD=PCH,DEV=0", "HDA Intel PCH, ALC3254 Analog"),
            ("plughw:CARD=PCH,DEV=0", "HDA Intel PCH, ALC3254 Analog"),
        ]);
        assert_eq!(out.len(), 1);
        // front: outranks surround/hw/plughw.
        assert_eq!(out[0].0, "front:CARD=PCH,DEV=0");
    }

    #[test]
    fn keeps_genuinely_distinct_outputs() {
        let out = run(&[
            (
                "default",
                "Default ALSA Output (currently PipeWire Media Server)",
            ),
            ("front:CARD=PCH,DEV=0", "HDA Intel PCH, ALC3254 Analog"),
            ("iec958:CARD=PCH,DEV=1", "HDA Intel PCH, ALC3254 Digital"),
            ("hdmi:CARD=PCH,DEV=3", "HDA Intel PCH, HDMI 0"),
            (
                "front:CARD=C20,DEV=0",
                "Cambridge Audio USB Audio 2.0, USB Audio",
            ),
            (
                "surround40:CARD=C20,DEV=0",
                "Cambridge Audio USB Audio 2.0, USB Audio",
            ),
        ]);
        let ids: Vec<&str> = out.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "default",
                "front:CARD=PCH,DEV=0",
                "iec958:CARD=PCH,DEV=1",
                "hdmi:CARD=PCH,DEV=3",
                "front:CARD=C20,DEV=0",
            ]
        );
    }

    #[test]
    fn passes_pipewire_node_names_through() {
        let out = run(&[
            ("alsa_output.usb-Cambridge", "alsa_output.usb-Cambridge"),
            (
                "alsa_output.pci-0000_00_1f.3",
                "alsa_output.pci-0000_00_1f.3",
            ),
        ]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn first_seen_order_is_preserved() {
        let out = run(&[
            (
                "hw:CARD=C20,DEV=0",
                "Cambridge Audio USB Audio 2.0, USB Audio",
            ),
            ("default", "Default ALSA Output"),
            // Better-ranked id for Cambridge appears later; it wins the group
            // but the group keeps its first-seen position (before Default).
            (
                "front:CARD=C20,DEV=0",
                "Cambridge Audio USB Audio 2.0, USB Audio",
            ),
        ]);
        let ids: Vec<&str> = out.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(ids, vec!["front:CARD=C20,DEV=0", "default"]);
    }

    #[test]
    fn drops_blank_displays() {
        let out = run(&[("weird", "   "), ("default", "Default ALSA Output")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "default");
    }
}
