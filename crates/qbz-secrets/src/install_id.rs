//! Persistent per-install UUID used as salt for the KDF fallback backend.
//!
//! The file is **not** a secret — it's a salt component. Writing it in
//! plaintext is fine. Its purpose is to ensure that two installations on
//! the same machine (e.g. system account + root) derive different keys,
//! and that moving the install directory to a different machine breaks
//! the derivation (because `machine-id` changes too).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use uuid::Uuid;

const INSTALL_ID_FILENAME: &str = "install-id";

pub(crate) fn load_or_create(storage_dir: &Path) -> std::io::Result<Uuid> {
    let path = storage_dir.join(INSTALL_ID_FILENAME);

    if let Ok(contents) = fs::read_to_string(&path) {
        if let Ok(uuid) = Uuid::parse_str(contents.trim()) {
            return Ok(uuid);
        }
        log::warn!(
            "[qbz-secrets] Corrupt install-id at {:?}, regenerating",
            path
        );
    }

    fs::create_dir_all(storage_dir)?;
    let uuid = Uuid::new_v4();
    let tmp_path: PathBuf = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(uuid.to_string().as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &path)?;
    log::info!("[qbz-secrets] Generated fresh install-id at {:?}", path);
    Ok(uuid)
}

/// Read the OS machine identifier. Uses platform-appropriate sources;
/// returns `None` if nothing reliable is available. A missing machine-id
/// is acceptable — the KDF falls back to just the install UUID, which is
/// still device-bound because the install directory is per-machine.
pub(crate) fn machine_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        for candidate in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(contents) = fs::read_to_string(candidate) {
                let trimmed = contents.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    {
        // IOPlatformUUID via ioreg; parse the "IOPlatformUUID" = "..." line
        use std::process::Command;
        let output = Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(idx) = line.find("IOPlatformUUID") {
                let tail = &line[idx..];
                if let Some(start) = tail.find('"').map(|i| i + 1) {
                    let rest = &tail[start..];
                    if let Some(end_off) = rest.find('"') {
                        return Some(rest[..end_off].to_string());
                    }
                    if let Some(end_off) = rest[rest.find('"').map(|i| i + 1).unwrap_or(0)..]
                        .find('"')
                    {
                        return Some(rest[..end_off].to_string());
                    }
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        // HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        // The shipped binary is a GUI subsystem image (W4), so it owns no
        // console; spawning `reg` without this flag makes Windows allocate one
        // and a black box flashes on screen at every startup. CREATE_NO_WINDOW
        // suppresses it. The command, its parse and its result are unchanged,
        // so no already-derived machine id moves.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let output = Command::new("reg")
            .creation_flags(CREATE_NO_WINDOW)
            .args([
                "query",
                "HKLM\\SOFTWARE\\Microsoft\\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("MachineGuid") {
                return line
                    .split_whitespace()
                    .last()
                    .map(str::to_string)
                    .filter(|s| !s.is_empty());
            }
        }
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}
