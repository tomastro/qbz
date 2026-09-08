use std::process::Command;

#[test]
fn bare_qbzd_prints_help_and_exits_2() {
    // 01-architecture.md §1.1: a typo'd verb must never leave a daemon running.
    let out = Command::new(env!("CARGO_BIN_EXE_qbzd")).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Usage"), "help text missing: {text}");
}

#[test]
fn version_answers_locally() {
    // 02-cli-and-api.md §2.2: `qbzd version` needs no daemon, no network.
    let out = Command::new(env!("CARGO_BIN_EXE_qbzd")).arg("version").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("api v1"));
}
