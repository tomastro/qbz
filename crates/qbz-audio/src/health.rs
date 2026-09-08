//! Audio-stack health probes + distro detection (HiFi wizard Slice 6 / check step).
//!
//! Cheap, read-only shell probes that tell the wizard what (if anything) is
//! missing on a Linux audio stack, plus `/etc/os-release` distro detection so
//! the check step can show the right `apt`/`dnf`/`pacman` remediation. None of
//! this opens a stream — purely diagnostic.

// Every target but Windows still probes; see `audio_stack_health`.
#[cfg(not(windows))]
use std::process::Command;

/// Result of the audio-stack probes. All best-effort; a failed probe reads as
/// `false` (the wizard then surfaces the matching remediation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStackHealth {
    /// WirePlumber session manager is running (`systemctl --user is-active`).
    pub wireplumber_active: bool,
    /// `pw-dump` is installed (native enumeration; `pipewire-bin`).
    pub has_pw_dump: bool,
    /// CPAL can see PipeWire through the ALSA bridge PCM (`aplay -L` lists
    /// `pipewire`). This needs `pipewire-alsa` and is required for PLAYBACK
    /// (the stream is opened via CPAL), not just enumeration.
    pub cpal_sees_pipewire: bool,
    /// `pactl` is available (Pulse compat path; `pulseaudio-utils`).
    pub has_pactl: bool,
    /// At least one audio sink is visible to `pw-dump`.
    pub any_devices: bool,
}

impl AudioStackHealth {
    /// Everything the wizard needs for bit-perfect playback is present.
    pub fn is_ready(&self) -> bool {
        self.wireplumber_active && self.cpal_sees_pipewire && self.any_devices
    }
}

/// True if `sh -c "<probe>"` exits 0. Windows never probes; see below.
#[cfg(not(windows))]
fn sh_ok(probe: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run the audio-stack probes. Linux-only meaningfully; elsewhere everything
/// reads false except where trivially true.
///
/// W14: on WINDOWS ONLY the answer is all-false, returned WITHOUT running the
/// probes. `systemctl`, `pactl` and `pw-dump` do not exist there (a spawn of a
/// missing image fails before a process is made, which is harmless), but `sh`
/// DOES exist wherever Git for Windows is installed, so the five `sh -c`
/// probes really run and flash five console windows past a GUI-subsystem
/// binary that owns no console.
///
/// Scoped to Windows, not to `not(linux)`: macOS and the BSDs can have a real
/// `pactl`/`pw-dump` from Homebrew or ports, and answering false for them
/// would be a lie this diff has no business telling.
#[cfg(windows)]
pub fn audio_stack_health() -> AudioStackHealth {
    AudioStackHealth {
        wireplumber_active: false,
        has_pw_dump: false,
        cpal_sees_pipewire: false,
        has_pactl: false,
        any_devices: false,
    }
}

/// Run the audio-stack probes. Linux-only meaningfully; elsewhere everything
/// reads false except where trivially true.
#[cfg(not(windows))]
pub fn audio_stack_health() -> AudioStackHealth {
    // systemd path first; fall back to a process check so non-systemd inits
    // (Gentoo/OpenRC, Void/runit) don't read as "WirePlumber down".
    let wireplumber_active = Command::new("systemctl")
        .args(["--user", "is-active", "wireplumber"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false)
        || sh_ok("pgrep -x wireplumber >/dev/null 2>&1");

    AudioStackHealth {
        wireplumber_active,
        has_pw_dump: sh_ok("command -v pw-dump >/dev/null 2>&1"),
        // `^pipewire$` line in `aplay -L` = the ALSA->PipeWire bridge PCM.
        cpal_sees_pipewire: sh_ok("aplay -L 2>/dev/null | grep -q '^pipewire$'"),
        has_pactl: sh_ok("command -v pactl >/dev/null 2>&1"),
        any_devices: sh_ok("pw-dump 2>/dev/null | grep -q 'Audio/Sink'"),
    }
}

/// Linux distribution family — drives the per-distro install commands. Order
/// matches the wizard's distro dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distro {
    Debian,
    /// Debian-based but systemd-free (sysVinit/runit).
    Antix,
    Fedora,
    Arch,
    /// Arch-based but systemd-free (OpenRC/runit/s6/dinit).
    Artix,
    OpenSuse,
    Gentoo,
    Void,
    /// Declarative — packages live in configuration.nix, init is systemd.
    NixOS,
    Other,
}

impl Distro {
    /// Dropdown order (index = position here). `Other` stays last.
    pub const ALL: [Distro; 10] = [
        Distro::Debian,
        Distro::Antix,
        Distro::Fedora,
        Distro::Arch,
        Distro::Artix,
        Distro::OpenSuse,
        Distro::Gentoo,
        Distro::Void,
        Distro::NixOS,
        Distro::Other,
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&d| d == self)
            .unwrap_or(Self::ALL.len() - 1)
    }

    /// Human label for the dropdown (mirrors the Tauri DistroSelector, plus the
    /// systemd-free families called out so the init-aware commands make sense).
    pub fn label(self) -> &'static str {
        match self {
            Distro::Debian => "Ubuntu / Debian / Mint / Pop!_OS",
            Distro::Antix => "antiX (systemd-free Debian)",
            Distro::Fedora => "Fedora / RHEL",
            Distro::Arch => "Arch / Manjaro / EndeavourOS",
            Distro::Artix => "Artix (systemd-free Arch)",
            Distro::OpenSuse => "openSUSE",
            Distro::Gentoo => "Gentoo / Funtoo",
            Distro::Void => "Void Linux",
            Distro::NixOS => "NixOS",
            Distro::Other => "Other",
        }
    }
}

/// The running init / service manager. Detected at RUNTIME — it is orthogonal
/// to the distro (Gentoo runs OpenRC *or* systemd; Debian runs systemd or
/// sysVinit/runit on antiX), so service commands must key off this, not the
/// distro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitSystem {
    Systemd,
    OpenRc,
    Runit,
    S6,
    Dinit,
    Unknown,
}

impl InitSystem {
    /// Dropdown order (index = position here). `Unknown` stays last.
    pub const ALL: [InitSystem; 6] = [
        InitSystem::Systemd,
        InitSystem::OpenRc,
        InitSystem::Runit,
        InitSystem::S6,
        InitSystem::Dinit,
        InitSystem::Unknown,
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&i| i == self)
            .unwrap_or(Self::ALL.len() - 1)
    }

    pub fn label(self) -> &'static str {
        match self {
            InitSystem::Systemd => "systemd",
            InitSystem::OpenRc => "OpenRC",
            InitSystem::Runit => "runit",
            InitSystem::S6 => "s6",
            InitSystem::Dinit => "dinit",
            InitSystem::Unknown => "Other / unknown",
        }
    }
}

/// App packaging sandbox. Inside one, host files (`/etc/os-release`, `/run`,
/// `/proc/1`) reflect the SANDBOX/runtime, not the user's host — so host
/// detection must read the host-exposed paths, and init detection can't be
/// trusted (it falls back to the manual override).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    None,
    Flatpak,
    Snap,
}

/// Detect the packaging sandbox: Flatpak exposes `/.flatpak-info`, Snap sets `$SNAP`.
pub fn detect_sandbox() -> Sandbox {
    if std::path::Path::new("/.flatpak-info").exists() {
        Sandbox::Flatpak
    } else if std::env::var_os("SNAP").is_some() {
        Sandbox::Snap
    } else {
        Sandbox::None
    }
}

/// Detect the running init system. The `/run/systemd/system` check is the
/// canonical `sd_booted()` test; the others mirror each supervisor's runtime
/// dir, with a `/proc/1/comm` fallback.
///
/// In a sandbox these all reflect the SANDBOX, not the host, so we return
/// `Unknown` and let the wizard's init override decide.
pub fn detect_init() -> InitSystem {
    if detect_sandbox() != Sandbox::None {
        return InitSystem::Unknown;
    }
    use std::path::Path;
    if Path::new("/run/systemd/system").exists() {
        return InitSystem::Systemd;
    }
    if Path::new("/run/openrc").exists() {
        return InitSystem::OpenRc;
    }
    if Path::new("/run/runit").exists() || Path::new("/etc/runit").exists() {
        return InitSystem::Runit;
    }
    if Path::new("/run/s6-rc").exists() || Path::new("/run/s6").exists() {
        return InitSystem::S6;
    }
    if Path::new("/run/dinitctl").exists() {
        return InitSystem::Dinit;
    }
    std::fs::read_to_string("/proc/1/comm")
        .map(|c| parse_init_from_comm(c.trim()))
        .unwrap_or(InitSystem::Unknown)
}

/// Pure classifier for PID 1's `comm` (testable fallback path).
fn parse_init_from_comm(comm: &str) -> InitSystem {
    match comm {
        "systemd" => InitSystem::Systemd,
        "openrc-init" | "openrc" => InitSystem::OpenRc,
        "runit" | "runsvdir" | "runit-init" => InitSystem::Runit,
        "s6-svscan" | "s6-linux-init" => InitSystem::S6,
        "dinit" => InitSystem::Dinit,
        _ => InitSystem::Unknown,
    }
}

/// Detect the distro from the HOST `os-release`, defaulting to `Other`.
///
/// Inside a sandbox the plain `/etc/os-release` is the runtime's (Flatpak
/// freedesktop-sdk) or the snap base's, NOT the user's distro — so read the
/// host-exposed path first: Flatpak guarantees `/run/host/os-release`, Snap
/// mounts the host root at `/var/lib/snapd/hostfs`. Falls back to `/etc`.
pub fn detect_distro() -> Distro {
    let host_path = match detect_sandbox() {
        Sandbox::Flatpak => Some("/run/host/os-release"),
        Sandbox::Snap => Some("/var/lib/snapd/hostfs/etc/os-release"),
        Sandbox::None => None,
    };
    let content = host_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .or_else(|| std::fs::read_to_string("/etc/os-release").ok());
    content.map(|c| parse_distro(&c)).unwrap_or(Distro::Other)
}

/// Pure `/etc/os-release` classifier (testable). Reads `ID` then `ID_LIKE`.
fn parse_distro(os_release: &str) -> Distro {
    let mut id = String::new();
    let mut id_like = String::new();
    for line in os_release.lines() {
        // os-release values may be bare, double-quoted, or single-quoted
        // (Gentoo uses single quotes), so strip both quote styles.
        if let Some(v) = line.strip_prefix("ID=") {
            id = v
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_lowercase();
        } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
            id_like = v
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_lowercase();
        }
    }
    let hay = format!("{} {}", id, id_like);
    let has = |needle: &str| hay.contains(needle);
    // Systemd-free derivatives MUST be matched before their parent family —
    // antiX has ID_LIKE=debian and Artix has ID_LIKE=arch, so the generic
    // checks below would otherwise swallow them and emit systemd commands.
    if has("antix") {
        Distro::Antix
    } else if has("artix") {
        Distro::Artix
    } else if has("nixos") {
        Distro::NixOS
    } else if has("ubuntu") || has("debian") || has("mint") || has("pop") {
        Distro::Debian
    } else if has("fedora") || has("rhel") || has("centos") {
        Distro::Fedora
    } else if has("arch") || has("manjaro") || has("endeavour") {
        Distro::Arch
    } else if has("suse") {
        Distro::OpenSuse
    } else if has("gentoo") {
        Distro::Gentoo
    } else if has("void") {
        Distro::Void
    } else {
        Distro::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_ubuntu_as_debian_family() {
        let os = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\n";
        assert_eq!(parse_distro(os), Distro::Debian);
    }

    #[test]
    fn classifies_via_id_like_when_id_unknown() {
        // Pop!_OS-style: ID=pop, ID_LIKE="ubuntu debian"
        let os = "ID=pop\nID_LIKE=\"ubuntu debian\"\n";
        assert_eq!(parse_distro(os), Distro::Debian);
        // EndeavourOS: ID=endeavouros, ID_LIKE=arch
        let os2 = "ID=endeavouros\nID_LIKE=arch\n";
        assert_eq!(parse_distro(os2), Distro::Arch);
    }

    #[test]
    fn classifies_fedora_arch_suse_gentoo_void_other() {
        assert_eq!(parse_distro("ID=fedora\n"), Distro::Fedora);
        assert_eq!(parse_distro("ID=arch\n"), Distro::Arch);
        assert_eq!(
            parse_distro("ID=opensuse-tumbleweed\nID_LIKE=\"suse opensuse\"\n"),
            Distro::OpenSuse
        );
        assert_eq!(parse_distro("ID=gentoo\n"), Distro::Gentoo);
        // Gentoo's real os-release single-quotes the value.
        assert_eq!(parse_distro("ID='gentoo'\n"), Distro::Gentoo);
        assert_eq!(parse_distro("ID=void\n"), Distro::Void);
        assert_eq!(parse_distro("ID=slackware\n"), Distro::Other);
        assert_eq!(parse_distro(""), Distro::Other);
    }

    #[test]
    fn systemd_free_derivatives_beat_their_parent_family() {
        // antiX: ID=antix, ID_LIKE=debian — must NOT classify as Debian.
        assert_eq!(parse_distro("ID=antix\nID_LIKE=debian\n"), Distro::Antix);
        // Artix: ID=artix, ID_LIKE=arch — must NOT classify as Arch.
        assert_eq!(parse_distro("ID=artix\nID_LIKE=arch\n"), Distro::Artix);
        // NixOS: ID=nixos.
        assert_eq!(parse_distro("ID=nixos\nID_LIKE=\"\"\n"), Distro::NixOS);
    }

    #[test]
    fn classifies_init_from_pid1_comm() {
        assert_eq!(parse_init_from_comm("systemd"), InitSystem::Systemd);
        assert_eq!(parse_init_from_comm("openrc-init"), InitSystem::OpenRc);
        assert_eq!(parse_init_from_comm("runit"), InitSystem::Runit);
        assert_eq!(parse_init_from_comm("s6-svscan"), InitSystem::S6);
        assert_eq!(parse_init_from_comm("dinit"), InitSystem::Dinit);
        assert_eq!(parse_init_from_comm("busybox"), InitSystem::Unknown);
    }

    #[test]
    fn distro_index_round_trips() {
        for d in Distro::ALL {
            assert_eq!(Distro::ALL[d.index()], d);
        }
    }
}
