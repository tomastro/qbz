//! Linux managed Auto: presentation proof before the parent constructs Qt.
//! Children never run auth, audio, IPC ownership or navigation recovery.

use super::*;
use std::process::{Command, Output};

const CHILD_ENV: &str = "QBZ_INTERNAL_AUTO_PREFLIGHT_CHILD";
const PROOF: &str = "QBZ_AUTO_PREFLIGHT_OK";
static SOFTWARE_NOTICE: AtomicBool = AtomicBool::new(false);

pub fn child_requested() -> bool {
    std::env::var(CHILD_ENV).is_ok_and(|value| value == "1")
}

pub fn run_child() -> i32 {
    std::env::remove_var(CHILD_ENV);
    // SAFETY: main calls only on the child's GUI thread, after constructing
    // its sole QGuiApplication. C++ owns and destroys the temporary window.
    let result = unsafe { qbz_qt_auto_preflight_window() };
    if result == 0 {
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{PROOF}");
        let _ = stdout.flush();
    }
    result
}

pub fn take_software_notice() -> bool {
    SOFTWARE_NOTICE.swap(false, Ordering::SeqCst)
}

fn external_backend(qsg: &str, quick: &str, qbz: &str) -> bool {
    !qsg.trim().is_empty()
        || !quick.trim().is_empty()
        || (!qbz.trim().is_empty() && !qbz.trim().eq_ignore_ascii_case("auto"))
}

fn command_for(executable: &std::path::Path, software: bool) -> Command {
    // Deliberately no parent arguments: a deep link belongs to the parent.
    // PRIME/ICD and QPA selection are inherited exactly as the parent uses them.
    let mut command = Command::new(executable);
    command.env(CHILD_ENV, "1");
    if software {
        command.env("QT_QUICK_BACKEND", "software");
    }
    command
}

fn proof(result: &(Output, bool)) -> Result<(), String> {
    let (output, timed_out) = result;
    if *timed_out {
        return Err("child exceeded the 8-second watchdog (killed and reaped)".into());
    }
    if output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == PROOF)
    {
        return Ok(());
    }
    Err(format!(
        "child status {}; presentation not proven; {}",
        output.status,
        preflight_output_tail(output)
    ))
}

/// At most Auto then software. A spawn/monitoring failure is an infrastructure
/// error, not proof of bad graphics; report it and stop without rewriting prefs.
fn decide(mut probe: impl FnMut(bool) -> Result<(Output, bool), String>) -> Result<bool, String> {
    match proof(&probe(false)?) {
        Ok(()) => Ok(false),
        Err(reason) => {
            log::warn!("[renderer] Auto presentation failed: {reason}; trying software once");
            proof(&probe(true)?)
                .map_err(|reason| format!("software presentation also failed: {reason}"))?;
            Ok(true)
        }
    }
}

/// Run after saved-GPU validation and apply_renderer_preference, but before
/// runtime/audio/QGuiApplication. Backend envs now include a forced saved
/// renderer, so that policy takes precedence too. No Vulkan enumeration here.
pub fn at_boot() -> Result<(), String> {
    let env = |name| std::env::var(name).unwrap_or_default();
    if external_backend(
        &env("QSG_RHI_BACKEND"),
        &env("QT_QUICK_BACKEND"),
        &env("QBZ_RENDERER"),
    ) {
        log::info!("[renderer] explicit backend policy preserved; managed Auto probe skipped");
        return Ok(());
    }
    if PREFLIGHT_APPROVED_GPU
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        // The parent will use this exact Vulkan identity, already presented.
        return Ok(());
    }
    if matches!(
        env("QT_QPA_PLATFORM").trim().split(':').next(),
        Some("offscreen" | "minimal" | "vnc")
    ) {
        log::info!("[renderer] non-desktop QPA; managed presentation probe skipped");
        return Ok(());
    }
    let executable =
        std::env::current_exe().map_err(|e| format!("cannot resolve executable: {e}"))?;
    let start = std::time::Instant::now();
    let software = decide(|software| {
        let started = std::time::Instant::now();
        let result = run_preflight_child(
            &mut command_for(&executable, software),
            GPU_PREFLIGHT_PARENT_TIMEOUT,
        );
        log::info!(
            "[renderer] {} presentation probe completed in {} ms",
            if software { "software" } else { "Auto" },
            started.elapsed().as_millis()
        );
        result
    })?;
    if software {
        // This launch only. No sentinel or preference changes, so a recovered
        // driver gets another Auto attempt on the next ordinary launch.
        std::env::set_var("QT_QUICK_BACKEND", "software");
        SOFTWARE_NOTICE.store(true, Ordering::SeqCst);
        log::warn!(
            "[renderer] software presentation confirmed; rescue for this launch only ({} ms total)",
            start.elapsed().as_millis()
        );
    } else {
        log::info!(
            "[renderer] Auto presentation confirmed; Qt default backend retained ({} ms total)",
            start.elapsed().as_millis()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn outcome(code: i32, marker: bool, timeout: bool) -> (Output, bool) {
        (
            Output {
                status: std::process::ExitStatus::from_raw(code),
                stdout: if marker {
                    format!("{PROOF}\n").into_bytes()
                } else {
                    Vec::new()
                },
                stderr: Vec::new(),
            },
            timeout,
        )
    }

    #[test]
    fn auto_health_failure_and_retry_are_launch_local_and_finite() {
        let mut attempts = Vec::new();
        assert!(!decide(|soft| {
            attempts.push(soft);
            Ok(outcome(0, true, false))
        })
        .unwrap());
        assert_eq!(attempts, [false]);
        for (code, marker, timeout) in [(6, false, false), (0, false, false), (0, true, true)] {
            attempts.clear();
            assert!(decide(|soft| {
                attempts.push(soft);
                Ok(if soft {
                    outcome(0, true, false)
                } else {
                    outcome(code, marker, timeout)
                })
            })
            .unwrap());
            assert_eq!(attempts, [false, true]);
        }
        attempts.clear();
        assert!(decide(|soft| {
            attempts.push(soft);
            Ok(outcome(6, false, false))
        })
        .is_err());
        assert_eq!(attempts, [false, true]);
        assert!(!decide(|_| Ok(outcome(0, true, false))).unwrap());
    }

    #[test]
    fn explicit_policy_ignores_empty_and_whitespace_values() {
        for value in ["", " ", "\t"] {
            assert!(!external_backend(value, value, value));
            assert!(!external_backend(value, value, " Auto "));
        }
        for (qsg, quick, qbz) in [("opengl", "", ""), ("", " software ", ""), ("", "", "gpu")] {
            assert!(external_backend(qsg, quick, qbz));
        }
    }

    #[test]
    fn a_spawn_failure_is_diagnostic_without_a_second_child() {
        let mut attempts = 0;
        let result = decide(|_| {
            attempts += 1;
            Err("fixture spawn failure".into())
        });
        assert_eq!(attempts, 1);
        assert!(result.unwrap_err().contains("spawn failure"));
        let mut missing = command_for(std::path::Path::new("/no such qbz executable"), false);
        assert!(run_preflight_child(&mut missing, std::time::Duration::from_millis(80)).is_err());
    }

    #[test]
    fn child_paths_args_logs_and_watchdog_are_isolated() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::Builder::new()
            .prefix("qbz preflight ")
            .tempdir()
            .unwrap();
        let executable = dir.path().join("fixture with spaces");
        symlink("/bin/sh", &executable).unwrap();
        let mut command = command_for(&executable, false);
        assert_eq!(
            command.get_args().count(),
            0,
            "parent links never reach the graphics child"
        );
        command.args(["-c", "test -z \"$XDG_ACTIVATION_TOKEN\" && test -z \"$DESKTOP_STARTUP_ID\" || exit 9; head -c 131072 /dev/zero >&2; echo QBZ_AUTO_PREFLIGHT_OK"]);
        let result = run_preflight_child(&mut command, std::time::Duration::from_secs(2)).unwrap();
        assert!(proof(&result).is_ok());
        assert_eq!(result.0.stderr.len(), 65536);

        // exec replaces the fixture shell: there is exactly one owned child,
        // just as the production command directly starts the QBZ executable.
        let pid_path = dir.path().join("pid");
        let mut command = command_for(&executable, false);
        command
            .env("FIXTURE_PID", &pid_path)
            .args(["-c", "echo $$ > \"$FIXTURE_PID\"; exec sleep 30"]);
        let start = std::time::Instant::now();
        let result =
            run_preflight_child(&mut command, std::time::Duration::from_millis(80)).unwrap();
        assert!(result.1);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        let pid = std::fs::read_to_string(pid_path).unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
            "timed-out child must be reaped"
        );
    }
}
