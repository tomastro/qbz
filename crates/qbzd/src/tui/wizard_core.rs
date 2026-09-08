// crates/qbzd/src/tui/wizard_core.rs — the frontend-agnostic HiFi-wizard logic.
//
// COPIED from crates/qbz-dac-wizard/src/lib.rs @ 7678bceb (the Slint controller)
// TODO(converge: dac-wizard) — fold into a shared crate (P1). The Slint crate
// depends on `slint` + `qbz-ui` (it drives `DacWizardState` globals), which the
// slint-free `qbzd` column must NOT link. So the window-free PURE pieces are
// copied here and the imperative `DacWizardState` plumbing (open_immediate /
// apply_health / recompute / apply_candidates / apply_configs / apply_poll / …)
// is re-expressed as plain data the TUI screen owns.
//
// Adaptations vs the original (all minimal, none touch the emitted config text):
//   1. `qbz_i18n::t("…")` / `tf(…)` calls dropped — the setup TUI is English-only
//      (03 §1.2), so caption literals are inlined (`.to_string()`); the plural
//      summary line is built by the screen, not here.
//   2. `DacCandidateData` / `DacConfigData` fields made `pub` so the sibling
//      screen module can read them (the original read them within one module).
//   3. `DacConfigData` gains `full_block()` / `target_paths()` / `short()` so the
//      TUI can render/copy/save one bordered box per DAC (the Slint accordion had
//      three sub-blocks + a separate created-paths list).
//   4. `seed_for_rate_depth()` added as the non-test call site for the copied
//      `track_matches_seed()` (the Slint side called it from the search-resolve
//      path, which the daemon test step does not reproduce).
// Everything else — remediation/reference command generation, the three config
// generators, slugify/short_name/rates_list, the test seeds — is verbatim.

use qbz_audio::{AudioStackHealth, Distro, InitSystem, NegotiatedRate};

// ── select-dacs (auto-detect + manual escape hatch) ────────────────────────

/// Plain, `Send` candidate produced on the worker thread.
pub struct DacCandidateData {
    pub id: String,
    pub description: String,
    pub bus: String,
    pub is_default: bool,
    pub looks_like_dac: bool,
    pub rates_label: String,
}

/// Heavy work (enumerate sinks via the pw-dump-robust path + probe rates for
/// the likely DACs). Runs on a blocking thread; returns plain data.
pub fn detect_blocking() -> Vec<DacCandidateData> {
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
        let description = if d.name.is_empty() { d.id.clone() } else { d.name };
        out.push(DacCandidateData {
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

/// Validate a manually-pasted node.name (escape hatch). 1:1 with the Tauri
/// `validateNodeName` / `detectDacType`.
pub fn validate_node_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty() && (t.contains("alsa_output") || t.contains("alsa_input"))
}

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
fn format_rates(rates: &[u32]) -> String {
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

// ── check step: audio-stack remediations (per distro / init) ────────────────

/// (caption, copy-paste command) per missing probe, for the given distro.
///
/// Service/restart commands are INIT-SYSTEM aware per distro (OpenRC on Gentoo,
/// runit on Void, systemd elsewhere), mirroring the Tauri DistroSelector
/// `restartCommands`. Installs and the restart are kept as separate blocks so
/// the multi-line Gentoo guidance never gets `&&`-joined.
pub fn remediations(h: AudioStackHealth, d: Distro, init: InitSystem) -> Vec<(String, String)> {
    // NixOS is declarative: you don't `apt/pacman install` pieces — you enable
    // the PipeWire module and rebuild. So collapse all the missing pieces into
    // one config block instead of per-package commands.
    if d == Distro::NixOS {
        if h.is_ready() {
            return Vec::new();
        }
        return vec![(
            "Enable PipeWire in your NixOS configuration".to_string(),
            NIXOS_PIPEWIRE_BLOCK.to_string(),
        )];
    }

    let mut out = Vec::new();
    let mut needs_restart = false;
    if !h.has_pw_dump {
        out.push((
            "Install the PipeWire tools (pw-dump)".to_string(),
            install(d, pkg_pw_tools(d)),
        ));
        needs_restart = true;
    }
    if !h.cpal_sees_pipewire {
        // THE Ubuntu no-list / no-playback bug: the ALSA->PipeWire bridge PCM.
        out.push((
            "Install the ALSA bridge so playback can reach PipeWire".to_string(),
            install(d, "pipewire-alsa"),
        ));
        needs_restart = true;
    }
    if !h.has_pactl {
        out.push((
            "Install the Pulse compatibility tools (optional fallback)".to_string(),
            install(d, pkg_pulse(d)),
        ));
        needs_restart = true;
    }
    if !h.any_devices {
        out.push((
            "No sinks detected — reinstall the ALSA UCM profiles, then reboot".to_string(),
            install_reinstall(d, "alsa-ucm-conf"),
        ));
    }
    // WirePlumber down, or we just installed something → (re)start the stack
    // with the ACTUAL init system running on this machine (not guessed from the
    // distro — Gentoo+systemd and Gentoo+OpenRC must differ).
    if !h.wireplumber_active || needs_restart {
        out.push((
            "(Re)start the PipeWire audio services".to_string(),
            restart_cmd(init).to_string(),
        ));
    }
    out
}

/// Init-system-aware "(re)start the audio services" command. PipeWire is a
/// user-session service, so only systemd has a first-class `--user` restart;
/// the others either use their user-service supervisor or a re-login.
pub fn restart_cmd(init: InitSystem) -> &'static str {
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
        Distro::NixOS => format!("# NixOS: add to configuration.nix (see the PipeWire block) — {pkgs}"),
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
pub fn reference_commands(d: Distro, init: InitSystem) -> Vec<(String, String)> {
    if d == Distro::NixOS {
        return vec![(
            "Enable PipeWire in your NixOS configuration".to_string(),
            NIXOS_PIPEWIRE_BLOCK.to_string(),
        )];
    }
    vec![
        (
            "Install the PipeWire audio stack".to_string(),
            install(d, full_stack_pkgs(d)),
        ),
        (
            "(Re)start the PipeWire audio services".to_string(),
            restart_cmd(init).to_string(),
        ),
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

// ── review-and-apply (per-DAC config generation) ────────────────────────────

/// Plain, `Send` per-DAC generated config (built on a worker thread).
pub struct DacConfigData {
    pub name: String,
    pub node_name: String,
    pub pipewire_conf: String,
    pub pulse_conf: String,
    pub wireplumber_conf: String,
}

impl DacConfigData {
    /// A short, filename-safe id for this DAC (slug of the description, else the
    /// node.name). Drives both the on-disk `~/.config/...` paths and the
    /// TUI's `w` save filename.
    pub fn short(&self) -> String {
        short_name(&self.name, &self.node_name)
    }

    /// The three target files this config populates (verbatim the path format
    /// the Slint `apply_configs` pushed to `created_paths`).
    pub fn target_paths(&self) -> Vec<String> {
        let short = self.short();
        vec![
            format!("~/.config/pipewire/pipewire.conf.d/99-qbz-dac-{short}.conf"),
            format!("~/.config/pipewire/client.conf.d/99-qbz-bitperfect-{short}.conf"),
            format!("~/.config/wireplumber/wireplumber.conf.d/99-qbz-dac-{short}.conf"),
        ]
    }

    /// The whole copy-paste block for this DAC: the three heredoc snippets
    /// (PipeWire rate-switching, per-app bit-perfect, WirePlumber node pin)
    /// joined so a single `c`/`w` reproduces every file.
    pub fn full_block(&self) -> String {
        [
            self.pipewire_conf.as_str(),
            self.pulse_conf.as_str(),
            self.wireplumber_conf.as_str(),
        ]
        .join("\n\n")
    }
}

/// Re-probe rates + build the three config snippets per DAC (blocking).
pub fn gen_configs_blocking(dacs: Vec<(String, String)>) -> Vec<DacConfigData> {
    dacs.into_iter()
        .map(|(node_name, name)| {
            let rates = qbz_audio::query_dac_capabilities(&node_name).sample_rates;
            let short = short_name(&name, &node_name);
            DacConfigData {
                pipewire_conf: pipewire_conf(&short, &rates),
                pulse_conf: pulse_conf(&short),
                wireplumber_conf: wireplumber_conf(&short, &node_name, &rates, &name),
                name,
                node_name,
            }
        })
        .collect()
}

/// The backup command shown above the generated blocks (back up the live
/// PipeWire/WirePlumber config before the operator applies anything).
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

// ── self-service playback test (N6 read-back) ───────────────────────────────

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
    let depth_ok = track.maximum_bit_depth.map(|d| d == seed.depth).unwrap_or(false);
    rate_ok && depth_ok
}

/// The curated reference seed a live `(rate_hz, depth)` pair corresponds to, if
/// any. Non-test call site for [`track_matches_seed`]: the daemon test step
/// reads a hardware rate back (`negotiated_active_rate`) rather than resolving a
/// seed track through search, so this labels the played rate as a known
/// bit-perfect reference when it lines up.
pub fn seed_for_rate_depth(rate_hz: u32, depth: u32) -> Option<&'static TestSeed> {
    let track = qbz_models::Track {
        maximum_sampling_rate: Some(rate_hz as f64),
        maximum_bit_depth: Some(depth),
        ..Default::default()
    };
    TEST_SEEDS.iter().find(|s| track_matches_seed(&track, s))
}

/// "192 kHz" / "44.1 kHz" from Hz (shared by the test read-back rendering).
pub fn khz(hz: u32) -> String {
    if hz % 1000 == 0 {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", hz as f64 / 1000.0)
    }
}

/// The DAC read-back line: real hardware rate · ALSA container format · channels
/// (N6). `S32_LE` = 24-bit carried in a 32-bit frame — this is the container, so
/// the wizard's "matched" verdict keys on the RATE, not the format string.
pub fn negotiated_label(n: &NegotiatedRate) -> String {
    format!("DAC: {} · {} · {} ch", khz(n.sample_rate), n.format, n.channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_node_names_like_tauri() {
        assert!(validate_node_name("alsa_output.usb-Cambridge-00.analog-stereo"));
        assert!(validate_node_name("alsa_input.pci-0000_00.analog-stereo"));
        assert!(!validate_node_name(""));
        assert!(!validate_node_name("   "));
        assert!(!validate_node_name("bluez_output.AA_BB"));
    }

    #[test]
    fn detects_dac_type() {
        assert_eq!(detect_dac_type("alsa_output.usb-Cambridge-00.analog-stereo"), "usb");
        assert_eq!(detect_dac_type("alsa_output.pci-0000_00_1f.3.analog-stereo"), "pci");
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
        assert_eq!(slugify("DacMagic Plus Analog Stereo"), "dacmagic-plus-analog-stereo");
        assert_eq!(slugify("Built-in Audio Analog Stereo"), "built-in-audio-analog-stereo");
        assert_eq!(slugify("  weird__name!! "), "weird-name");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn wireplumber_conf_pins_node_and_rates() {
        let c = wireplumber_conf("dacmagic", "alsa_output.usb-x.analog-stereo", &[44100, 192000], "DacMagic");
        assert!(c.contains("node.name = \"alsa_output.usb-x.analog-stereo\""));
        assert!(c.contains("audio.allowed-rates = [ 44100 192000 ]"));
        assert!(c.contains("99-qbz-dac-dacmagic.conf"));
        assert!(c.contains("resample.disable = true"));
    }

    #[test]
    fn full_block_and_paths_cover_the_three_files() {
        let cfg = DacConfigData {
            name: "DacMagic".to_string(),
            node_name: "alsa_output.usb-x.analog-stereo".to_string(),
            pipewire_conf: "PW".to_string(),
            pulse_conf: "PULSE".to_string(),
            wireplumber_conf: "WP".to_string(),
        };
        assert_eq!(cfg.short(), "dacmagic");
        let block = cfg.full_block();
        assert!(block.contains("PW") && block.contains("PULSE") && block.contains("WP"));
        let paths = cfg.target_paths();
        assert_eq!(paths.len(), 3);
        assert!(paths[0].contains("pipewire.conf.d/99-qbz-dac-dacmagic.conf"));
        assert!(paths[1].contains("client.conf.d/99-qbz-bitperfect-dacmagic.conf"));
        assert!(paths[2].contains("wireplumber.conf.d/99-qbz-dac-dacmagic.conf"));
    }

    #[test]
    fn seed_lookup_matches_known_reference_rates() {
        // 24/192 → Toto "Africa"; 16/44100 → George Harrison.
        assert_eq!(seed_for_rate_depth(192000, 24).map(|s| s.title), Some("Africa"));
        assert_eq!(seed_for_rate_depth(44100, 16).map(|s| s.title), Some("My Sweet Lord"));
        // The two 44.1 seeds differ only by depth.
        assert_eq!(seed_for_rate_depth(44100, 24).map(|s| s.title), Some("LUNCH"));
        // An off-grid rate matches nothing.
        assert!(seed_for_rate_depth(48000, 24).is_none());
    }

    #[test]
    fn remediations_nixos_collapses_to_one_config_block() {
        let unhealthy = AudioStackHealth {
            wireplumber_active: false,
            has_pw_dump: false,
            cpal_sees_pipewire: false,
            has_pactl: false,
            any_devices: false,
        };
        let r = remediations(unhealthy, Distro::NixOS, InitSystem::Systemd);
        assert_eq!(r.len(), 1);
        assert!(r[0].1.contains("services.pipewire"));
        assert!(r[0].1.contains("nixos-rebuild switch"));
    }

    #[test]
    fn remediations_debian_names_the_alsa_bridge_and_is_init_aware() {
        let missing_bridge = AudioStackHealth {
            wireplumber_active: true,
            has_pw_dump: true,
            cpal_sees_pipewire: false, // the Ubuntu empty-list bug
            has_pactl: true,
            any_devices: true,
        };
        let r = remediations(missing_bridge, Distro::Debian, InitSystem::Systemd);
        assert!(r.iter().any(|(_, cmd)| cmd == "sudo apt install pipewire-alsa"));
        // needs_restart flipped → an init-aware systemd restart block is appended.
        assert!(r.iter().any(|(_, cmd)| cmd.contains("systemctl --user restart")));
    }

    #[test]
    fn reference_commands_used_in_sandbox_full_stack() {
        let r = reference_commands(Distro::Debian, InitSystem::Systemd);
        assert_eq!(r.len(), 2);
        assert!(r[0].1.contains("pipewire-alsa"), "full stack must include the ALSA bridge");
        assert!(r[1].1.contains("systemctl --user restart"));
    }

    #[test]
    fn negotiated_label_shows_rate_format_channels() {
        let n = NegotiatedRate { sample_rate: 192000, format: "S32_LE".to_string(), channels: 2 };
        assert_eq!(negotiated_label(&n), "DAC: 192 kHz · S32_LE · 2 ch");
    }
}
