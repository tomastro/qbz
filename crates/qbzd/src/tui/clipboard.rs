// crates/qbzd/src/tui/clipboard.rs — SSH-first tiered clipboard for the wizard's
// config blocks (FB4). The daemon's primary operator drives it over SSH, so
// OSC 52 (a terminal escape the SSH client's terminal honours) leads there;
// on a local session the native tools (`wl-copy`/`xclip`) lead because local
// terminals often don't honour OSC 52. Every path ends at a file save so a copy
// NEVER errors out of the flow — the operator is always told which tier worked.
//
// Two pure, unit-tested pieces: `osc52_payload` (base64 + optional tmux
// passthrough wrapping) and `plan_tiers` (env-driven ordering).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// One clipboard mechanism, in preference groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// OSC 52 escape written to the controlling tty — works across SSH.
    Osc52,
    /// `wl-copy` (Wayland).
    WlCopy,
    /// `xclip -selection clipboard` (X11).
    Xclip,
    /// Guaranteed fallback: write to `~/qbzd-wizard/<name>.conf`.
    File,
}

impl Tier {
    /// The per-block flash shown after a copy/save attempt. OSC 52 is
    /// one-way — the local terminal may silently ignore the escape, and
    /// there is no ack to confirm it landed — so its flash says "sent …
    /// paste to confirm" rather than claiming a completed copy. wl-copy and
    /// xclip keep the checkmark (they're locally verifiable-ish: the process
    /// exited 0 having read the pipe); File keeps naming the artifact.
    pub fn short_label(self) -> &'static str {
        match self {
            Tier::Osc52 => "sent via OSC 52 — paste to confirm",
            Tier::WlCopy => "copied ✓ (wl-copy)",
            Tier::Xclip => "copied ✓ (xclip)",
            Tier::File => "copied ✓ (saved to file)",
        }
    }
}

/// The clipboard-relevant environment, sampled once. Pure input to `plan_tiers`
/// so the ordering is testable without touching the real environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipEnv {
    pub ssh: bool,
    pub tmux: bool,
    pub wayland: bool,
    pub x11: bool,
}

impl ClipEnv {
    /// Read the live environment: SSH_TTY/SSH_CONNECTION, TMUX, WAYLAND_DISPLAY,
    /// DISPLAY.
    pub fn from_env() -> Self {
        let has = |k: &str| std::env::var_os(k).map(|v| !v.is_empty()).unwrap_or(false);
        ClipEnv {
            ssh: has("SSH_TTY") || has("SSH_CONNECTION"),
            tmux: has("TMUX"),
            wayland: has("WAYLAND_DISPLAY"),
            x11: has("DISPLAY"),
        }
    }
}

/// Ordered tiers to attempt, SSH-first. Remote (SSH/tmux) leads with OSC 52
/// since it is the one that survives the hop; a local session leads with the
/// native tool for the running display server. Always ends at `File` so a copy
/// can never fail out of the flow.
pub fn plan_tiers(env: &ClipEnv) -> Vec<Tier> {
    let remote = env.ssh || env.tmux;
    let mut tiers = Vec::new();
    if remote {
        tiers.push(Tier::Osc52);
    } else {
        if env.wayland {
            tiers.push(Tier::WlCopy);
        }
        if env.x11 {
            tiers.push(Tier::Xclip);
        }
        tiers.push(Tier::Osc52);
    }
    tiers.push(Tier::File);
    tiers
}

/// Standard base64 with padding (no external crate — keeps the slim `qbzd`
/// dependency set, and the payload builder must stay pure/testable).
fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Build the OSC 52 clipboard-set escape for `data`. When `tmux` is true, wrap
/// it in tmux's DCS passthrough (`\ePtmux;…\e\\`) with every inner ESC doubled,
/// so the sequence reaches the outer terminal instead of being swallowed by
/// tmux. Base64 keeps arbitrary bytes chunk-safe on the wire.
pub fn osc52_payload(data: &str, tmux: bool) -> String {
    let b64 = base64(data.as_bytes());
    let seq = format!("\x1b]52;c;{b64}\x07");
    if tmux {
        let inner = seq.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{inner}\x1b\\")
    } else {
        seq
    }
}

/// OSC 52's payload cap, applied to the base64-encoded blob (post-inflation,
/// which is what actually crosses the wire). Terminals differ wildly in what
/// they accept for a clipboard-set escape — many silently truncate (or just
/// drop) anything past a few tens of KB — so past this ceiling `copy` skips
/// the tier outright rather than risk a truncated, silently-wrong paste.
/// 100 KB is generous but bounded. Kept as a pure fn so the threshold
/// decision is unit-tested without a tty.
const OSC52_MAX_B64_LEN: usize = 100 * 1024;

/// Whether a base64-encoded OSC 52 payload of `b64_len` bytes is small enough
/// to attempt over OSC 52 (`> 100 KB` post-base64 skips the tier).
pub fn osc52_fits(b64_len: usize) -> bool {
    b64_len <= OSC52_MAX_B64_LEN
}

/// The directory the `w` (write) action and the file-fallback tier save into —
/// ALWAYS under the operator's home, NEVER a system path (HARD RULE: the wizard
/// never writes a live config file).
pub fn wizard_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join("qbzd-wizard")
}

/// Write `text` to `~/qbzd-wizard/<stem>.conf`, creating the dir. Returns the
/// full path so the caller can print it verbatim.
pub fn write_wizard_file(stem: &str, text: &str) -> std::io::Result<PathBuf> {
    let dir = wizard_dir();
    std::fs::create_dir_all(&dir)?;
    let safe: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let stem = if safe.trim_matches('-').is_empty() { "dac".to_string() } else { safe };
    let path = dir.join(format!("{stem}.conf"));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// What a copy attempt did, so the operator is always told which tier worked.
pub struct CopyReport {
    pub tier: Tier,
    /// A one-line, human message ("copied to clipboard (OSC 52)", "clipboard
    /// unavailable — saved to <path>").
    pub detail: String,
}

/// Copy `text` to the clipboard via the best available tier. `stem` names the
/// file-fallback save. Never returns an error: the last tier is a file write,
/// and even if THAT fails the report says so rather than losing the flow.
pub fn copy(text: &str, stem: &str, env: &ClipEnv) -> CopyReport {
    // Skip the OSC 52 tier outright when the payload is too big for a
    // terminal to reliably accept — no tty write is even attempted.
    let oversized = !osc52_fits(base64(text.as_bytes()).len());
    for tier in plan_tiers(env) {
        if tier == Tier::Osc52 && oversized {
            continue; // too large for OSC 52 — go straight to the next tier
        }
        match try_tier(tier, text, stem, env.tmux) {
            Some(detail) => {
                // Osc52 always sits directly before File in plan_tiers, so
                // when it was skipped for size, name that reason instead of
                // File's generic "clipboard unavailable" (which covers the
                // headless / no-wl-copy-or-xclip case too).
                let detail = if oversized && tier == Tier::File {
                    detail.replacen("clipboard unavailable", "too large for OSC 52", 1)
                } else {
                    detail
                };
                return CopyReport { tier, detail };
            }
            None => continue,
        }
    }
    // plan_tiers always ends in File; reaching here means even the file write
    // failed. Report it — do not panic, do not lose the operator's flow.
    CopyReport {
        tier: Tier::File,
        detail: "could not copy or save the block".to_string(),
    }
}

/// Attempt one tier. `Some(detail)` on success, `None` to fall through.
fn try_tier(tier: Tier, text: &str, stem: &str, tmux: bool) -> Option<String> {
    match tier {
        Tier::Osc52 => write_osc52(text, tmux).ok().map(|()| {
            // Honest wording: the escape reached the tty, but OSC 52 is
            // one-way — plenty of terminals ignore it by default — so this
            // is NOT "copied", it's "sent, unconfirmed".
            if tmux {
                "sent via OSC 52 (tmux) — paste to confirm".to_string()
            } else {
                "sent via OSC 52 — paste to confirm".to_string()
            }
        }),
        Tier::WlCopy => pipe_to("wl-copy", &[], text)
            .then(|| "copied to clipboard (wl-copy)".to_string()),
        Tier::Xclip => pipe_to("xclip", &["-selection", "clipboard"], text)
            .then(|| "copied to clipboard (xclip)".to_string()),
        Tier::File => write_wizard_file(stem, text)
            .ok()
            .map(|p| format!("clipboard unavailable — saved to {}", p.display())),
    }
}

/// Write the OSC 52 escape to the controlling tty (`/dev/tty`) so it reaches the
/// terminal even under the ratatui alt-screen (it is an escape, not drawn text).
fn write_osc52(text: &str, tmux: bool) -> std::io::Result<()> {
    let payload = osc52_payload(text, tmux);
    let mut tty = std::fs::OpenOptions::new().write(true).open("/dev/tty")?;
    tty.write_all(payload.as_bytes())?;
    tty.flush()
}

/// Spawn `cmd args`, feed `text` on stdin, discard its output. Both `wl-copy`
/// and `xclip` fork a background holder after reading stdin, so `wait()`
/// returns promptly. `true` on spawn+write+exit success.
fn pipe_to(cmd: &str, args: &[&str], text: &str) -> bool {
    let child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return false, // not installed → fall through
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    // Dropping stdin closes the pipe; the holder daemon detaches, so wait() ends.
    matches!(child.wait(), Ok(status) if status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_payload_wraps_base64_in_the_escape() {
        let p = osc52_payload("foobar", false);
        assert_eq!(p, "\x1b]52;c;Zm9vYmFy\x07");
    }

    #[test]
    fn osc52_payload_tmux_passthrough_doubles_esc() {
        let p = osc52_payload("foobar", true);
        // DCS passthrough envelope, ST terminator, inner ESC doubled.
        assert!(p.starts_with("\x1bPtmux;"));
        assert!(p.ends_with("\x1b\\"));
        // The inner OSC introducer's ESC is doubled inside the envelope
        // (`;` from the `Ptmux;` prefix, then the doubled ESC, then the OSC).
        assert!(p.contains(";\x1b\x1b]52;c;Zm9vYmFy\x07"));
        // Exactly the envelope's two structural ESCs remain single: the leading
        // `\x1bP` and the trailing `\x1b\\`; every ESC in between is doubled, so
        // no single ESC is adjacent to a non-ESC in the inner body.
        assert_eq!(p.matches('\x1b').count(), 4); // \x1bP + \x1b\x1b + \x1b\\
        // The exact wrapped payload.
        assert_eq!(p, "\x1bPtmux;\x1b\x1b]52;c;Zm9vYmFy\x07\x1b\\");
    }

    #[test]
    fn plan_tiers_is_ssh_first_remote() {
        let ssh = ClipEnv { ssh: true, tmux: false, wayland: true, x11: true };
        assert_eq!(plan_tiers(&ssh), vec![Tier::Osc52, Tier::File]);
        let tmux = ClipEnv { ssh: false, tmux: true, wayland: false, x11: false };
        assert_eq!(plan_tiers(&tmux), vec![Tier::Osc52, Tier::File]);
    }

    #[test]
    fn plan_tiers_prefers_native_tools_locally() {
        let wayland = ClipEnv { ssh: false, tmux: false, wayland: true, x11: true };
        assert_eq!(plan_tiers(&wayland), vec![Tier::WlCopy, Tier::Xclip, Tier::Osc52, Tier::File]);
        let x11 = ClipEnv { ssh: false, tmux: false, wayland: false, x11: true };
        assert_eq!(plan_tiers(&x11), vec![Tier::Xclip, Tier::Osc52, Tier::File]);
        let headless = ClipEnv { ssh: false, tmux: false, wayland: false, x11: false };
        assert_eq!(plan_tiers(&headless), vec![Tier::Osc52, Tier::File]);
    }

    #[test]
    fn osc52_fits_thresholds_at_100kb_post_base64() {
        assert!(osc52_fits(0));
        assert!(osc52_fits(100 * 1024)); // exactly the cap still fits
        assert!(!osc52_fits(100 * 1024 + 1)); // one byte over skips the tier
    }

    #[test]
    fn plan_tiers_always_ends_in_file() {
        for ssh in [false, true] {
            for tmux in [false, true] {
                for wayland in [false, true] {
                    for x11 in [false, true] {
                        let env = ClipEnv { ssh, tmux, wayland, x11 };
                        assert_eq!(*plan_tiers(&env).last().unwrap(), Tier::File);
                    }
                }
            }
        }
    }
}
