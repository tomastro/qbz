//! The one property this crate exists for, and the one no in-process test can
//! demonstrate: a secret wrapped by one run of the app can be unwrapped by the
//! next.
//!
//! Everything else here passes whether or not that holds. A single process can
//! wrap and unwrap against a master key that never left memory and never
//! reached a store, which is exactly what keyring's `mock` backend hands back
//! when no platform feature is enabled — silently, while `Backend::new` reports
//! `BackendKind::Keyring`. That shipped on macOS and Windows and cost every
//! offline download its content key on the next launch.
//!
//! So this test crosses a process boundary. It re-executes the test binary,
//! wraps in the child, then re-executes again and unwraps in a second child.
//!
//! Deliberately **backend-agnostic**: it asserts persistence, not which store
//! provided it. The KDF fallback satisfies it too (its install id is on disk),
//! so a runner with no secret-service is a pass rather than a flake — while the
//! mock, which satisfies neither, fails everywhere.

use std::path::Path;
use std::process::Command;

/// Set on the children. Its value is the half of the exchange to perform.
const ROLE: &str = "QBZ_SECRETS_RESTART_ROLE";
const SERVICE: &str = "QBZ_SECRETS_RESTART_SERVICE";
const DIR: &str = "QBZ_SECRETS_RESTART_DIR";

const SECRET: &[u8; 16] = b"0123456789abcdef";

#[test]
fn a_wrapped_secret_survives_a_process_restart() {
    if let Ok(role) = std::env::var(ROLE) {
        run_child(&role);
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    // Unique per run so concurrent jobs — and a developer's real qbz entry —
    // never collide. Cleaned up at the end.
    let service = format!(
        "qbz-secrets-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );

    let wrapped = spawn_self("wrap", &service, dir.path());
    assert!(
        wrapped.status.success(),
        "the first run could not wrap a secret:\n{}",
        String::from_utf8_lossy(&wrapped.stderr)
    );

    let unwrapped = spawn_self("unwrap", &service, dir.path());
    let stderr = String::from_utf8_lossy(&unwrapped.stderr).into_owned();

    forget(&service);

    assert!(
        unwrapped.status.success(),
        "a secret wrapped by one process could not be unwrapped by the next — \
         the master key did not outlive the process that made it:\n{stderr}"
    );
}

/// Re-execute this test binary with `ROLE` set, so the child runs the body
/// above and takes the branch at the top.
fn spawn_self(role: &str, service: &str, dir: &Path) -> std::process::Output {
    Command::new(std::env::current_exe().expect("current_exe"))
        .args([
            "--exact",
            "a_wrapped_secret_survives_a_process_restart",
            "--nocapture",
        ])
        .env(ROLE, role)
        .env(SERVICE, service)
        .env(DIR, dir)
        .output()
        .expect("re-exec the test binary")
}

fn run_child(role: &str) {
    let service = std::env::var(SERVICE).expect(SERVICE);
    let dir = std::env::var(DIR).expect(DIR);
    let blob = Path::new(&dir).join("wrapped.bin");

    let vault = qbz_secrets::SecretBox::open(&service, Path::new(&dir))
        .unwrap_or_else(|e| panic!("open the vault: {e}"));
    eprintln!("child role={role} backend={:?}", vault.backend_kind());

    match role {
        "wrap" => {
            let wrapped = vault.wrap(SECRET).unwrap_or_else(|e| panic!("wrap: {e}"));
            std::fs::write(&blob, wrapped).expect("write the wrapped blob");
        }
        _ => {
            let wrapped = std::fs::read(&blob).expect("read the wrapped blob");
            let plain = vault
                .unwrap(&wrapped)
                .unwrap_or_else(|e| panic!("unwrap: {e}"));
            assert_eq!(plain, SECRET, "the secret came back changed");
        }
    }
}

/// Drop the entry this run created. Best-effort: the KDF fallback never made
/// one, and a store that refuses the delete is not this test's business.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn forget(service: &str) {
    if let Ok(entry) = keyring::Entry::new(service, "master-key-v1") {
        let _ = entry.delete_credential();
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn forget(_service: &str) {}
