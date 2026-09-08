// crates/qbzd/src/cli/service.rs — `qbzd service [systemd|openrc|runit]`, a
// pure-local generator (no daemon, like `completions`) that prints a ready-to-
// install service definition for the host's init system.
//
// systemd is the standard and ships a user unit already; this generator also
// covers the inits the standard packaging can't: OpenRC (the owner's) and runit.
// The one thing those get wrong by default is the AUDIO ENVIRONMENT — a
// system-level service drops to a user but loses that user's session env, so
// PipeWire/Pulse (via `XDG_RUNTIME_DIR`) and the config/token roots (via `HOME`)
// go missing. Every non-user template sets both explicitly, resolved for the
// target user at generation time (`getent`/`id`), so the daemon finds the same
// audio stack it would in an interactive session. (An ALSA-direct/bit-perfect
// setup doesn't need `XDG_RUNTIME_DIR` at all — it's harmless there and correct
// for the PipeWire case.)
use std::process::Command;

/// `qbzd service [INIT] [--user U] [--bin PATH] [--system]`. Prints the unit to
/// stdout (pipe/redirect it into place); install steps go to stderr so stdout
/// stays clean. Exit 0, or 2 on an unknown/undetectable init.
pub fn service(init: Option<String>, user: Option<String>, bin: Option<String>, system: bool) -> i32 {
    let init = match init.map(|s| s.to_ascii_lowercase()).or_else(detect_init) {
        Some(i) => i,
        None => {
            eprintln!("error: could not detect the init system — name it explicitly");
            eprintln!("  → qbzd service systemd | openrc | runit");
            return 2;
        }
    };

    let t = resolve(user, bin);
    let (file, hint) = match init.as_str() {
        "systemd" if system => (systemd_system(&t), systemd_system_hint()),
        "systemd" => (systemd_user(&t), systemd_user_hint()),
        "openrc" => (openrc(&t), openrc_hint()),
        "runit" => (runit(&t), runit_hint()),
        other => {
            eprintln!("error: unknown init system '{other}'");
            eprintln!("  → systemd | openrc | runit");
            return 2;
        }
    };

    print!("{file}");
    eprint!("{hint}");
    0
}

// ============================ target resolution ============================

struct Target {
    user: String,
    group: String,
    uid: String,
    home: String,
    xdg_runtime: String,
    bin: String,
}

fn resolve(user: Option<String>, bin: Option<String>) -> Target {
    let bin = bin
        .or_else(|| std::env::current_exe().ok().and_then(|p| p.to_str().map(String::from)))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "/usr/bin/qbzd".to_string());

    let user = user
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .unwrap_or_else(|| "qbz".to_string());

    let (uid, home) = passwd(&user);
    let group = id_group(&user).unwrap_or_else(|| user.clone());

    // For the CURRENT user, the live XDG_RUNTIME_DIR is authoritative (captures a
    // non-default path); for another user, derive it from the uid.
    let is_current = std::env::var("USER").ok().as_deref() == Some(user.as_str());
    let xdg_runtime = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| is_current && !s.is_empty())
        .unwrap_or_else(|| format!("/run/user/{uid}"));

    Target { user, group, uid, home, xdg_runtime, bin }
}

/// `(uid, home)` for a user via `getent passwd` (name:x:uid:gid:gecos:home:sh),
/// falling back to `id -u` + a `/home/<user>` heuristic on a passwd-less box.
fn passwd(user: &str) -> (String, String) {
    if let Some(line) = run(&["getent", "passwd", user]) {
        let f: Vec<&str> = line.trim().split(':').collect();
        if f.len() >= 7 && !f[2].is_empty() {
            return (f[2].to_string(), f[5].to_string());
        }
    }
    let uid = run(&["id", "-u", user]).unwrap_or_else(|| "1000".to_string());
    (uid, format!("/home/{user}"))
}

fn id_group(user: &str) -> Option<String> {
    run(&["id", "-gn", user]).filter(|s| !s.is_empty())
}

/// Run a command and return its trimmed stdout on success, else None.
fn run(args: &[&str]) -> Option<String> {
    let out = Command::new(args[0]).args(&args[1..]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn detect_init() -> Option<String> {
    let exists = |p: &str| std::path::Path::new(p).exists();
    if exists("/run/systemd/system") {
        Some("systemd".into())
    } else if exists("/run/openrc") || exists("/sbin/openrc") || exists("/etc/init.d/functions.sh") {
        Some("openrc".into())
    } else if exists("/run/runit") || exists("/etc/runit") || exists("/etc/sv") {
        Some("runit".into())
    } else {
        None
    }
}

// ============================ templates ============================

fn systemd_user(t: &Target) -> String {
    format!(
        "# qbzd.service — QBZ headless Qobuz playback daemon (systemd USER unit).\n\
         #\n\
         # REQUIRED on a headless box: sudo loginctl enable-linger {user}\n\
         #   Without linger this unit stops when you log out of SSH and the\n\
         #   device vanishes from the Qobuz app. `qbzd status` warns when off.\n\
         # A user unit inherits your session env, so PipeWire/ALSA just work.\n\
         [Unit]\n\
         Description=QBZ headless Qobuz playback daemon\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={bin} run\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        user = t.user,
        bin = t.bin,
    )
}

fn systemd_system(t: &Target) -> String {
    format!(
        "# qbzd.service — QBZ headless Qobuz playback daemon (systemd SYSTEM unit).\n\
         #\n\
         # Runs as {user}. XDG_RUNTIME_DIR must exist — enable linger so the\n\
         # user's /run/user/{uid} (and PipeWire) come up at boot:\n\
         #   sudo loginctl enable-linger {user}\n\
         [Unit]\n\
         Description=QBZ headless Qobuz playback daemon\n\
         After=network-online.target sound.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User={user}\n\
         Environment=HOME={home}\n\
         Environment=XDG_RUNTIME_DIR={xdg}\n\
         ExecStart={bin} run\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        user = t.user,
        uid = t.uid,
        home = t.home,
        xdg = t.xdg_runtime,
        bin = t.bin,
    )
}

fn openrc(t: &Target) -> String {
    format!(
        "#!/sbin/openrc-run\n\
         # qbzd — QBZ headless Qobuz playback daemon (OpenRC).\n\
         #\n\
         # Runs as {user} under supervise-daemon (auto-restart on crash). Audio\n\
         # needs the user's runtime dir + HOME; /run/user/{uid} is provided by\n\
         # elogind for a logged-in or LINGERING user. Make sure {user} is in the\n\
         # `audio` group for direct ALSA/bit-perfect access.\n\
         \n\
         description=\"QBZ headless Qobuz playback daemon\"\n\
         \n\
         supervisor=\"supervise-daemon\"\n\
         command=\"{bin}\"\n\
         command_args=\"run\"\n\
         command_user=\"{user}:{group}\"\n\
         pidfile=\"/run/${{RC_SVCNAME}}.pid\"\n\
         respawn_delay=10\n\
         \n\
         start_pre() {{\n\
         \tHOME=\"{home}\"\n\
         \tXDG_RUNTIME_DIR=\"{xdg}\"\n\
         \texport HOME XDG_RUNTIME_DIR\n\
         }}\n\
         \n\
         depend() {{\n\
         \tneed localmount\n\
         \tafter bootmisc elogind\n\
         \tuse net dns logger\n\
         }}\n",
        user = t.user,
        group = t.group,
        uid = t.uid,
        home = t.home,
        xdg = t.xdg_runtime,
        bin = t.bin,
    )
}

fn runit(t: &Target) -> String {
    format!(
        "#!/bin/sh\n\
         # /etc/sv/qbzd/run — QBZ headless Qobuz playback daemon (runit).\n\
         #\n\
         # Runs as {user}. Audio needs the user's runtime dir + HOME; /run/user/\n\
         # {uid} must exist (elogind/seatd for a logged-in or lingering user).\n\
         # {user} should be in the `audio` group for direct ALSA/bit-perfect.\n\
         exec 2>&1\n\
         export HOME=\"{home}\"\n\
         export XDG_RUNTIME_DIR=\"{xdg}\"\n\
         exec chpst -u {user}:{group} {bin} run\n",
        user = t.user,
        group = t.group,
        uid = t.uid,
        home = t.home,
        xdg = t.xdg_runtime,
        bin = t.bin,
    )
}

// ============================ install hints (stderr) ============================

fn systemd_user_hint() -> String {
    "\n# Install (user unit):\n\
     #   qbzd service systemd > ~/.config/systemd/user/qbzd.service\n\
     #   systemctl --user daemon-reload\n\
     #   systemctl --user enable --now qbzd\n\
     #   sudo loginctl enable-linger \"$USER\"   # REQUIRED on a headless box\n"
        .to_string()
}

fn systemd_system_hint() -> String {
    "\n# Install (system unit):\n\
     #   qbzd service systemd --system | sudo tee /etc/systemd/system/qbzd.service > /dev/null\n\
     #   sudo systemctl daemon-reload\n\
     #   sudo systemctl enable --now qbzd\n"
        .to_string()
}

fn openrc_hint() -> String {
    "\n# Install:\n\
     #   qbzd service openrc | sudo tee /etc/init.d/qbzd > /dev/null\n\
     #   sudo chmod +x /etc/init.d/qbzd\n\
     #   sudo rc-update add qbzd default\n\
     #   sudo rc-service qbzd start\n"
        .to_string()
}

fn runit_hint() -> String {
    "\n# Install:\n\
     #   sudo mkdir -p /etc/sv/qbzd\n\
     #   qbzd service runit | sudo tee /etc/sv/qbzd/run > /dev/null\n\
     #   sudo chmod +x /etc/sv/qbzd/run\n\
     #   sudo ln -s /etc/sv/qbzd /var/service/    # Void; Artix: /run/runit/service\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Target {
        Target {
            user: "alice".into(),
            group: "alice".into(),
            uid: "1001".into(),
            home: "/home/alice".into(),
            xdg_runtime: "/run/user/1001".into(),
            bin: "/usr/local/bin/qbzd".into(),
        }
    }

    #[test]
    fn systemd_user_has_execstart_and_no_user_env() {
        let u = systemd_user(&t());
        assert!(u.contains("ExecStart=/usr/local/bin/qbzd run"));
        assert!(u.contains("WantedBy=default.target"));
        // A user unit must NOT hardcode User=/XDG_RUNTIME_DIR (it's the session's).
        assert!(!u.contains("User="));
        assert!(!u.contains("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn system_templates_carry_the_audio_env_for_the_target_user() {
        for tpl in [systemd_system(&t()), openrc(&t()), runit(&t())] {
            assert!(tpl_has(&tpl, "/run/user/1001"), "missing XDG_RUNTIME_DIR:\n{tpl}");
            assert!(tpl_has(&tpl, "/home/alice"), "missing HOME:\n{tpl}");
            assert!(tpl_has(&tpl, "alice"), "missing user:\n{tpl}");
            // The bin + a `run` invocation, however each init spells it (systemd/
            // runit put them adjacent; openrc splits command/command_args).
            assert!(tpl_has(&tpl, "/usr/local/bin/qbzd"), "missing bin:\n{tpl}");
            assert!(tpl_has(&tpl, "run"), "missing run:\n{tpl}");
        }
    }

    fn tpl_has(s: &str, needle: &str) -> bool {
        s.contains(needle)
    }

    #[test]
    fn openrc_uses_supervise_daemon_and_drops_to_the_user() {
        let o = openrc(&t());
        assert!(o.starts_with("#!/sbin/openrc-run"));
        assert!(o.contains("supervisor=\"supervise-daemon\""));
        assert!(o.contains("command_user=\"alice:alice\""));
    }

    #[test]
    fn runit_execs_via_chpst_under_the_user() {
        let r = runit(&t());
        assert!(r.starts_with("#!/bin/sh"));
        assert!(r.contains("exec chpst -u alice:alice /usr/local/bin/qbzd run"));
    }
}
