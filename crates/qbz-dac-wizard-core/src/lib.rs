//! HiFi Wizard (DAC setup) — the frontend-agnostic core.
//!
//! # Why this crate exists
//!
//! The retired Slint wizard wrote directly into frontend globals. This crate is
//! the surviving logic, returning plain data: `qbz-qt` drives it through
//! `dac_wizard_qt.rs`. ADR-006, frontend-agnostic core.
//!
//! `qbzd` duplicates parts of this in `tui/wizard_core.rs`. That one is live
//! code and remains a genuine candidate to fold onto this crate.
//!
//! # What is guaranteed
//!
//! READ-ONLY. Nothing here writes a system file, opens a stream or runs a
//! command. It probes (`qbz_audio::health`, sink enumeration, rate queries)
//! and it formats text. That property is the wizard's whole safety story and
//! it is enforced by construction: this crate has no `std::process::Command`
//! and no write-side `std::fs`.

use qbz_audio::{AudioStackHealth, Distro, InitSystem, Sandbox};

// ===========================================================================
// Environment detection (thin re-exports so a frontend needs ONE dependency)
// ===========================================================================

/// Distro dropdown options, in `Distro::ALL` order (index = the dropdown's).
pub fn distro_options() -> Vec<String> {
    Distro::ALL.iter().map(|d| d.label().to_string()).collect()
}

/// Init-system dropdown options, in `InitSystem::ALL` order.
pub fn init_options() -> Vec<String> {
    InitSystem::ALL
        .iter()
        .map(|i| i.label().to_string())
        .collect()
}

/// Auto-detected distro, as a dropdown index.
pub fn detected_distro_index() -> i32 {
    qbz_audio::detect_distro().index() as i32
}

/// Auto-detected init system, as a dropdown index.
pub fn detected_init_index() -> i32 {
    qbz_audio::detect_init().index() as i32
}

/// `("Flatpak" | "Snap" | "")` — empty when the process is not sandboxed.
pub fn sandbox_name() -> &'static str {
    match qbz_audio::detect_sandbox() {
        Sandbox::Flatpak => "Flatpak",
        Sandbox::Snap => "Snap",
        Sandbox::None => "",
    }
}

/// Blocking audio-stack probe (shells out through `qbz_audio`; call it off the
/// UI thread).
pub fn probe_health() -> AudioStackHealth {
    qbz_audio::audio_stack_health()
}

fn distro_at(index: i32) -> Distro {
    Distro::ALL
        .get(index.max(0) as usize)
        .copied()
        .unwrap_or(Distro::Other)
}

fn init_at(index: i32) -> InitSystem {
    InitSystem::ALL
        .get(index.max(0) as usize)
        .copied()
        .unwrap_or(InitSystem::Unknown)
}

// ===========================================================================
// Check step — health verdict + per-distro remediations
// ===========================================================================

/// One "you're missing X" fix: a caption and a copy-paste command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remediation {
    pub caption: String,
    pub command: String,
}

/// Everything the check step renders, for one (health, distro, init) triple.
#[derive(Clone, Debug, Default)]
pub struct CheckView {
    pub health_ok: bool,
    /// Empty in a sandbox — there is no verdict to give, only reference
    /// commands (the UI shows its sandbox banner instead).
    pub summary: String,
    pub remediations: Vec<Remediation>,
}

/// Build the check step from a completed probe and the two dropdown indices
/// (either of which the user may have overridden).
///
/// In a sandbox the host probes are blind, so no verdict is rendered — the
/// caller gets reference setup commands for the chosen distro/init instead
/// (Tauri-style, which never probed either).
pub fn check_view(
    health: AudioStackHealth,
    distro_index: i32,
    init_index: i32,
    sandboxed: bool,
) -> CheckView {
    let distro = distro_at(distro_index);
    let init = init_at(init_index);

    if sandboxed {
        return CheckView {
            health_ok: false,
            summary: String::new(),
            remediations: reference_commands(distro, init),
        };
    }

    let rows = remediations(health, distro, init);
    let ok = health.is_ready();
    let summary = if ok {
        qbz_i18n::t("Your audio stack is ready for bit-perfect playback.")
    } else {
        let n = rows.len();
        qbz_i18n::tf(
            "{} item needs attention before bit-perfect playback will work.",
            "{} items need attention before bit-perfect playback will work.",
            n as i64,
            &[&n.to_string()],
        )
    };
    CheckView {
        health_ok: ok,
        summary,
        remediations: rows,
    }
}

/// (caption, copy-paste command) per missing probe, for the given distro.
///
/// Service/restart commands are INIT-SYSTEM aware per distro (OpenRC on Gentoo,
/// runit on Void, systemd elsewhere), mirroring the Tauri DistroSelector
/// `restartCommands`. Installs and the restart are kept as separate blocks so
/// the multi-line Gentoo guidance never gets `&&`-joined.
fn remediations(h: AudioStackHealth, d: Distro, init: InitSystem) -> Vec<Remediation> {
    // NixOS is declarative: you don't `apt/pacman install` pieces — you enable
    // the PipeWire module and rebuild. So collapse all the missing pieces into
    // one config block instead of per-package commands.
    if d == Distro::NixOS {
        if h.is_ready() {
            return Vec::new();
        }
        return vec![Remediation {
            caption: qbz_i18n::t("Enable PipeWire in your NixOS configuration"),
            command: NIXOS_PIPEWIRE_BLOCK.to_string(),
        }];
    }

    let mut out = Vec::new();
    let mut needs_restart = false;
    if !h.has_pw_dump {
        out.push(Remediation {
            caption: qbz_i18n::t("Install the PipeWire tools (pw-dump)"),
            command: install(d, pkg_pw_tools(d)),
        });
        needs_restart = true;
    }
    if !h.cpal_sees_pipewire {
        // THE Ubuntu no-list / no-playback bug: the ALSA->PipeWire bridge PCM.
        out.push(Remediation {
            caption: qbz_i18n::t("Install the ALSA bridge so playback can reach PipeWire"),
            command: install(d, "pipewire-alsa"),
        });
        needs_restart = true;
    }
    if !h.has_pactl {
        out.push(Remediation {
            caption: qbz_i18n::t("Install the Pulse compatibility tools (optional fallback)"),
            command: install(d, pkg_pulse(d)),
        });
        needs_restart = true;
    }
    if !h.any_devices {
        out.push(Remediation {
            caption: qbz_i18n::t(
                "No sinks detected — reinstall the ALSA UCM profiles, then reboot",
            ),
            command: install_reinstall(d, "alsa-ucm-conf"),
        });
    }
    // WirePlumber down, or we just installed something → (re)start the stack
    // with the ACTUAL init system running on this machine (not guessed from the
    // distro — Gentoo+systemd and Gentoo+OpenRC must differ).
    if !h.wireplumber_active || needs_restart {
        out.push(Remediation {
            caption: qbz_i18n::t("(Re)start the PipeWire audio services"),
            command: restart_cmd(init).to_string(),
        });
    }
    out
}

/// Init-system-aware "(re)start the audio services" command. PipeWire is a
/// user-session service, so only systemd has a first-class `--user` restart;
/// the others either use their user-service supervisor or a re-login.
fn restart_cmd(init: InitSystem) -> &'static str {
    match init {
        InitSystem::Systemd => "systemctl --user restart pipewire pipewire-pulse wireplumber",
        InitSystem::OpenRc => {
            "# OpenRC: PipeWire runs in your user session, not as an OpenRC service.\n\
             # Log out and back in to restart it."
        }
        InitSystem::Runit => {
            "sv restart pipewire wireplumber   # if set up as runit user services; otherwise log out and back in"
        }
        InitSystem::S6 => "# s6: restart via your supervision tree, or log out and back in",
        InitSystem::Dinit => "dinitctl restart pipewire wireplumber   # or log out and back in",
        InitSystem::Unknown => "# Restart PipeWire via your init system, or log out and back in",
    }
}

/// The restart command for a dropdown index — the review step's step 3.
pub fn restart_command_for(init_index: i32) -> String {
    restart_cmd(init_at(init_index)).to_string()
}

fn pkg_pw_tools(d: Distro) -> &'static str {
    match d {
        // Debian-family (incl. antiX) ship pw-* in pipewire-bin.
        Distro::Debian | Distro::Antix => "pipewire-bin",
        Distro::Fedora => "pipewire-utils",
        // Arch (incl. Artix) / openSUSE / Gentoo / Void ship pw-* with pipewire.
        _ => "pipewire",
    }
}

fn pkg_pulse(d: Distro) -> &'static str {
    match d {
        Distro::Debian | Distro::Antix => "pipewire-pulse pulseaudio-utils",
        Distro::Fedora => "pipewire-pulseaudio",
        _ => "pipewire-pulse",
    }
}

fn install(d: Distro, pkgs: &str) -> String {
    match d {
        // Package manager is a property of the distro family, NOT the init.
        Distro::Debian | Distro::Antix => format!("sudo apt install {pkgs}"),
        Distro::Fedora => format!("sudo dnf install {pkgs}"),
        Distro::Arch | Distro::Artix => format!("sudo pacman -S {pkgs}"),
        Distro::OpenSuse => format!("sudo zypper install {pkgs}"),
        Distro::Gentoo => format!("sudo emerge {pkgs}   # package name may differ on Gentoo"),
        Distro::Void => format!("sudo xbps-install -S {pkgs}"),
        // NixOS is special-cased in remediations(); this is an unreached fallback.
        Distro::NixOS => {
            format!("# NixOS: add to configuration.nix (see the PipeWire block) — {pkgs}")
        }
        Distro::Other => format!("Install with your package manager: {pkgs}"),
    }
}

fn install_reinstall(d: Distro, pkg: &str) -> String {
    match d {
        Distro::Debian | Distro::Antix => format!("sudo apt install --reinstall {pkg}"),
        Distro::Fedora => format!("sudo dnf reinstall {pkg}"),
        _ => install(d, pkg),
    }
}

const NIXOS_PIPEWIRE_BLOCK: &str = "# /etc/nixos/configuration.nix:\n\
     services.pipewire = {\n\
     \u{20}\u{20}enable = true;\n\
     \u{20}\u{20}alsa.enable = true;\n\
     \u{20}\u{20}pulse.enable = true;\n\
     \u{20}\u{20}wireplumber.enable = true;\n\
     };\n\
     # then apply:\n\
     sudo nixos-rebuild switch";

/// Full reference setup commands for the chosen distro/init, shown when QBZ
/// can't probe the host (sandbox). Mirrors the Tauri DistroSelector, which
/// always showed per-distro install + restart commands (no probing).
fn reference_commands(d: Distro, init: InitSystem) -> Vec<Remediation> {
    if d == Distro::NixOS {
        return vec![Remediation {
            caption: qbz_i18n::t("Enable PipeWire in your NixOS configuration"),
            command: NIXOS_PIPEWIRE_BLOCK.to_string(),
        }];
    }
    vec![
        Remediation {
            caption: qbz_i18n::t("Install the PipeWire audio stack"),
            command: install(d, full_stack_pkgs(d)),
        },
        Remediation {
            caption: qbz_i18n::t("(Re)start the PipeWire audio services"),
            command: restart_cmd(init).to_string(),
        },
    ]
}

/// The full recommended package set (incl. `pipewire-alsa`, the bit the old
/// Tauri list omitted — the cause of the Ubuntu empty-list bug).
fn full_stack_pkgs(d: Distro) -> &'static str {
    match d {
        Distro::Debian | Distro::Antix => {
            "pipewire pipewire-pulse pipewire-alsa wireplumber alsa-utils"
        }
        Distro::Fedora => "pipewire pipewire-pulseaudio pipewire-alsa wireplumber alsa-utils",
        Distro::Arch | Distro::Artix => {
            "pipewire pipewire-pulse pipewire-alsa wireplumber alsa-utils"
        }
        Distro::OpenSuse => "pipewire pipewire-pulseaudio pipewire-alsa wireplumber alsa-utils",
        Distro::Gentoo => "media-video/pipewire media-video/wireplumber media-sound/alsa-utils",
        Distro::Void => "pipewire wireplumber alsa-utils",
        Distro::NixOS => "",
        Distro::Other => "pipewire pipewire-pulse wireplumber alsa-utils",
    }
}

// ===========================================================================
// Select-DACs step — enumeration + the manual escape hatch
// ===========================================================================

/// One enumerated sink the user can pick to configure.
#[derive(Clone, Debug, Default)]
pub struct DacCandidate {
    /// PipeWire `node.name` — the value the manual escape hatch asks for.
    pub id: String,
    /// Pretty name, e.g. "DacMagic Plus Analog Stereo".
    pub description: String,
    /// "usb" | "pci" | "bluetooth" | ""
    pub bus: String,
    pub is_default: bool,
    /// hardware && bus ∈ {usb,pci} → pre-selected.
    pub looks_like_dac: bool,
    /// Pre-filled supported rates, e.g. "44.1 / 96 / 192 kHz".
    pub rates_label: String,
}

/// Enumerate sinks via the pw-dump-robust path and probe rates for the likely
/// DACs. BLOCKING — call it off the UI thread.
pub fn detect_blocking() -> Vec<DacCandidate> {
    let devices = qbz_audio::backend::BackendManager::create_backend(
        qbz_audio::backend::AudioBackendType::PipeWire,
    )
    .and_then(|b| b.enumerate_devices())
    .unwrap_or_default();

    let mut out = Vec::new();
    for d in devices {
        let bus = d.device_bus.unwrap_or_default();
        let looks_like_dac = d.is_hardware && (bus == "usb" || bus == "pci");
        // Only probe rates for likely DACs (skip virtual/monitor sinks).
        let rates_label = if looks_like_dac {
            format_rates(&qbz_audio::query_dac_capabilities(&d.id).sample_rates)
        } else {
            String::new()
        };
        let description = if d.name.is_empty() {
            d.id.clone()
        } else {
            d.name
        };
        out.push(DacCandidate {
            id: d.id,
            description,
            bus,
            is_default: d.is_default,
            looks_like_dac,
            rates_label,
        });
    }
    out
}

/// Validate a manually-pasted `node.name`. 1:1 with the Tauri
/// `validateNodeName`.
pub fn validate_node_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty() && (t.contains("alsa_output") || t.contains("alsa_input"))
}

/// Classify a pasted node name. 1:1 with the Tauri `detectDacType`.
pub fn detect_dac_type(name: &str) -> &'static str {
    let l = name.to_lowercase();
    if l.contains("usb-") || l.contains(".usb") {
        "usb"
    } else if l.contains("pci-") || l.contains(".pci") {
        "pci"
    } else if l.contains("bluez") || l.contains("bluetooth") {
        "bluetooth"
    } else if l.contains("virtual") || l.contains("null") || l.contains("dummy") {
        "virtual"
    } else {
        "unknown"
    }
}

/// "44.1 / 96 / 192 kHz" from a rate list (kHz, .1 only when non-integer).
pub fn format_rates(rates: &[u32]) -> String {
    if rates.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = rates
        .iter()
        .map(|&r| {
            if r % 1000 == 0 {
                format!("{}", r / 1000)
            } else {
                format!("{:.1}", r as f64 / 1000.0)
            }
        })
        .collect();
    format!("{} kHz", parts.join(" / "))
}

// ===========================================================================
// Test step — the four curated tracks + the N6 read-back
// ===========================================================================

/// One curated test track (owner-provided). Resolved by id-hint first, then by
/// "artist title" search if the id 404s (a pulled license) — never raw-id-only.
pub struct TestSeed {
    pub depth: u32,
    pub rate: f64,
    pub id_hint: u64,
    pub artist: &'static str,
    pub title: &'static str,
}

pub const TEST_SEEDS: [TestSeed; 4] = [
    TestSeed { depth: 16, rate: 44100.0, id_hint: 19301386, artist: "George Harrison", title: "My Sweet Lord" },
    TestSeed { depth: 24, rate: 44100.0, id_hint: 266725027, artist: "Billie Eilish", title: "LUNCH" },
    TestSeed { depth: 24, rate: 96000.0, id_hint: 126886854, artist: "Iron Maiden", title: "Stratego" },
    TestSeed { depth: 24, rate: 192000.0, id_hint: 52265, artist: "Toto", title: "Africa" },
];

/// True if a resolved track matches this seed's family (rate + bit depth — the
/// two 44.1 seeds only differ by depth).
pub fn track_matches_seed(track: &qbz_models::Track, seed: &TestSeed) -> bool {
    let rate_ok = track
        .maximum_sampling_rate
        .map(|r| (r * 1000.0 - seed.rate).abs() < 1.0 || (r - seed.rate).abs() < 1.0)
        .unwrap_or(false);
    let depth_ok = track
        .maximum_bit_depth
        .map(|d| d == seed.depth)
        .unwrap_or(false);
    rate_ok && depth_ok
}

/// The two read-back lines plus the truth signal, for one poll.
#[derive(Clone, Debug, Default)]
pub struct PollView {
    pub requested_label: String,
    pub negotiated_label: String,
    /// The DAC's real clock matches what QBZ asked for.
    pub rate_matched: bool,
}

/// Format one poll: the rate QBZ requested vs the DAC's real negotiated rate
/// (N6).
pub fn poll_view(
    requested_rate: u32,
    requested_bits: u32,
    negotiated: Option<qbz_audio::NegotiatedRate>,
) -> PollView {
    let requested_label = if requested_rate > 0 {
        qbz_i18n::t_args(
            "QBZ requesting {} · {}-bit",
            &[&khz(requested_rate), &requested_bits.to_string()],
        )
    } else {
        qbz_i18n::t("Nothing playing")
    };
    match negotiated {
        Some(n) => PollView {
            requested_label,
            // The DAC's REAL hardware params (N6): rate + ALSA container format
            // (e.g. S32_LE = 24-bit in a 32-bit frame) + channels. This is the
            // bit-perfect proof — exactly what the hardware is clocked at.
            negotiated_label: qbz_i18n::t_args(
                "DAC: {} · {} · {} ch",
                &[&khz(n.sample_rate), &n.format, &n.channels.to_string()],
            ),
            rate_matched: requested_rate > 0 && n.sample_rate == requested_rate,
        },
        None => PollView {
            requested_label,
            negotiated_label: qbz_i18n::t("Waiting for the DAC to start playing…"),
            rate_matched: false,
        },
    }
}

fn khz(hz: u32) -> String {
    if hz % 1000 == 0 {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", hz as f64 / 1000.0)
    }
}

// ===========================================================================
// Review-and-apply step — per-DAC config generation
// ===========================================================================

/// One selected DAC's generated copy-paste config (read-only).
#[derive(Clone, Debug, Default)]
pub struct DacConfig {
    pub name: String,
    pub node_name: String,
    pub pipewire_conf: String,
    pub pulse_conf: String,
    pub wireplumber_conf: String,
}

/// Re-probe rates + build the three config snippets per DAC. BLOCKING.
pub fn gen_configs_blocking(dacs: Vec<(String, String)>) -> Vec<DacConfig> {
    dacs.into_iter()
        .map(|(node_name, name)| {
            let rates = qbz_audio::query_dac_capabilities(&node_name).sample_rates;
            let short = short_name(&name, &node_name);
            DacConfig {
                pipewire_conf: pipewire_conf(&short, &rates),
                pulse_conf: pulse_conf(&short),
                wireplumber_conf: wireplumber_conf(&short, &node_name, &rates, &name),
                name,
                node_name,
            }
        })
        .collect()
}

/// The three `~/.config/...` paths one generated config would create — the
/// done step's "Config files you can create" list, in the same order the
/// review step shows the snippets.
pub fn created_paths(configs: &[DacConfig]) -> Vec<String> {
    let mut paths = Vec::new();
    for d in configs {
        let short = short_name(&d.name, &d.node_name);
        paths.push(format!(
            "~/.config/pipewire/pipewire.conf.d/99-qbz-dac-{short}.conf"
        ));
        paths.push(format!(
            "~/.config/pipewire/client.conf.d/99-qbz-bitperfect-{short}.conf"
        ));
        paths.push(format!(
            "~/.config/wireplumber/wireplumber.conf.d/99-qbz-dac-{short}.conf"
        ));
    }
    paths
}

pub const BACKUP_CMD: &str = "BACKUP=~/.config/qbz/backups/pipewire-$(date +%Y%m%d-%H%M%S)\nmkdir -p \"$BACKUP\"\ncp -a ~/.config/pipewire \"$BACKUP/\" 2>/dev/null || true\ncp -a ~/.config/wireplumber \"$BACKUP/\" 2>/dev/null || true\necho \"Backup created at: $BACKUP\"";

/// A short, filename-safe DAC name: slug of the description, else the node.name.
fn short_name(name: &str, node_name: &str) -> String {
    let slug = slugify(name);
    if !slug.is_empty() {
        return slug;
    }
    let nslug = slugify(node_name);
    if nslug.is_empty() {
        "dac".to_string()
    } else {
        nslug
    }
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn rates_list(rates: &[u32]) -> String {
    if rates.is_empty() {
        "44100 48000 88200 96000 176400 192000".to_string()
    } else {
        rates
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn pipewire_conf(short: &str, rates: &[u32]) -> String {
    let rates = rates_list(rates);
    [
        "mkdir -p ~/.config/pipewire/pipewire.conf.d".to_string(),
        format!("cat > ~/.config/pipewire/pipewire.conf.d/99-qbz-dac-{short}.conf << 'EOF'"),
        "# QBZ DAC Setup - Sample Rate Switching".to_string(),
        "context.properties = {".to_string(),
        format!("  default.clock.allowed-rates = [ {rates} ]"),
        "}".to_string(),
        "EOF".to_string(),
    ]
    .join("\n")
}

fn pulse_conf(short: &str) -> String {
    [
        "mkdir -p ~/.config/pipewire/client.conf.d".to_string(),
        format!("cat > ~/.config/pipewire/client.conf.d/99-qbz-bitperfect-{short}.conf << 'EOF'"),
        "# QBZ DAC Setup - Per-App Bit-Perfect".to_string(),
        "stream.rules = [".to_string(),
        "  {".to_string(),
        "    matches = [".to_string(),
        "      { application.process.binary = \"qbz\" }".to_string(),
        "      { application.name = \"PipeWire ALSA [qbz]\" }".to_string(),
        "    ]".to_string(),
        "    actions = { update-props = { resample.disable = true, channelmix.disable = true } }"
            .to_string(),
        "  }".to_string(),
        "]".to_string(),
        "EOF".to_string(),
    ]
    .join("\n")
}

fn wireplumber_conf(short: &str, node_name: &str, rates: &[u32], description: &str) -> String {
    let rates = rates_list(rates);
    [
        "mkdir -p ~/.config/wireplumber/wireplumber.conf.d".to_string(),
        format!("cat > ~/.config/wireplumber/wireplumber.conf.d/99-qbz-dac-{short}.conf << 'EOF'"),
        format!("# QBZ DAC Setup - {description}"),
        "monitor.alsa.rules = [".to_string(),
        "  {".to_string(),
        "    matches = [".to_string(),
        format!("      {{ node.name = \"{node_name}\", media.class = \"Audio/Sink\" }}"),
        "    ]".to_string(),
        "    actions = {".to_string(),
        "      update-props = {".to_string(),
        format!("        audio.allowed-rates = [ {rates} ]"),
        "        resample.disable = true".to_string(),
        "        channelmix.disable = true".to_string(),
        "      }".to_string(),
        "    }".to_string(),
        "  }".to_string(),
        "]".to_string(),
        "EOF".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_node_names_like_tauri() {
        assert!(validate_node_name(
            "alsa_output.usb-Cambridge-00.analog-stereo"
        ));
        assert!(validate_node_name("alsa_input.pci-0000_00.analog-stereo"));
        assert!(!validate_node_name(""));
        assert!(!validate_node_name("   "));
        assert!(!validate_node_name("bluez_output.AA_BB"));
    }

    #[test]
    fn detects_dac_type() {
        assert_eq!(
            detect_dac_type("alsa_output.usb-Cambridge-00.analog-stereo"),
            "usb"
        );
        assert_eq!(
            detect_dac_type("alsa_output.pci-0000_00_1f.3.analog-stereo"),
            "pci"
        );
        assert_eq!(detect_dac_type("bluez_output.AA"), "bluetooth");
        assert_eq!(detect_dac_type("alsa_output.virtual-dummy"), "virtual");
        assert_eq!(detect_dac_type("something.else"), "unknown");
    }

    #[test]
    fn formats_rates_khz() {
        assert_eq!(format_rates(&[44100, 96000, 192000]), "44.1 / 96 / 192 kHz");
        assert_eq!(format_rates(&[]), "");
    }

    #[test]
    fn slugifies_descriptions() {
        assert_eq!(
            slugify("DacMagic Plus Analog Stereo"),
            "dacmagic-plus-analog-stereo"
        );
        assert_eq!(
            slugify("Built-in Audio Analog Stereo"),
            "built-in-audio-analog-stereo"
        );
        assert_eq!(slugify("  weird__name!! "), "weird-name");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn wireplumber_conf_pins_node_and_rates() {
        let c = wireplumber_conf(
            "dacmagic",
            "alsa_output.usb-x.analog-stereo",
            &[44100, 192000],
            "DacMagic",
        );
        assert!(c.contains("node.name = \"alsa_output.usb-x.analog-stereo\""));
        assert!(c.contains("audio.allowed-rates = [ 44100 192000 ]"));
        assert!(c.contains("99-qbz-dac-dacmagic.conf"));
        assert!(c.contains("resample.disable = true"));
    }

    /// The done step's path list must line up with the snippets the review step
    /// shows — three paths per DAC, in the pipewire / client / wireplumber
    /// order the accordions use.
    #[test]
    fn created_paths_are_three_per_dac_in_order() {
        let cfg = DacConfig {
            name: "DacMagic Plus".to_string(),
            node_name: "alsa_output.usb-x.analog-stereo".to_string(),
            ..Default::default()
        };
        let paths = created_paths(&[cfg]);
        assert_eq!(paths.len(), 3);
        assert!(paths[0].contains("pipewire.conf.d/99-qbz-dac-dacmagic-plus.conf"));
        assert!(paths[1].contains("client.conf.d/99-qbz-bitperfect-dacmagic-plus.conf"));
        assert!(paths[2].contains("wireplumber.conf.d/99-qbz-dac-dacmagic-plus.conf"));
    }

    /// A sandbox has no verdict to give: no summary, and the rows are the
    /// reference install + restart rather than probe-derived fixes.
    #[test]
    fn sandbox_renders_reference_commands_and_no_verdict() {
        // A fully-healthy probe, so the assertion cannot pass by accident: in
        // a sandbox the verdict is suppressed REGARDLESS of what was probed.
        let health = AudioStackHealth {
            wireplumber_active: true,
            has_pw_dump: true,
            cpal_sees_pipewire: true,
            has_pactl: true,
            any_devices: true,
        };
        let view = check_view(health, 0, 0, true);
        assert!(view.summary.is_empty());
        assert!(!view.health_ok);
        assert_eq!(view.remediations.len(), 2);
    }
}
