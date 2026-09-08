//! Lazy singleton access to the `qbz-secrets::SecretBox` for the offline
//! cache.
//!
//! The vault is opened on first use (with the OS-keyring + KDF-fallback
//! selection happening inside `qbz-secrets`) and then shared across all
//! offline-cache calls. The storage directory is the app config dir — the
//! install UUID for the KDF fallback lives there so it survives upgrades.

use std::path::Path;
use std::sync::OnceLock;

use qbz_secrets::{SecretBox, SecretError};

const SERVICE_NAME: &str = "qbz";

static VAULT: OnceLock<std::sync::Mutex<Option<SecretBox>>> = OnceLock::new();

/// Get-or-init the vault. `storage_dir` is used only on the first call;
/// subsequent calls return the cached handle regardless.
pub fn get_or_init(storage_dir: &Path) -> Result<SecretBox, SecretError> {
    let cell = VAULT.get_or_init(|| std::sync::Mutex::new(None));
    // Recover from a poisoned mutex rather than wedging the vault for the rest
    // of the session: a panic during a prior `SecretBox::open` (e.g. the zbus
    // "runtime within a runtime" panic this guards against) leaves the inner
    // `Option` as `None`, so recovering simply retries the open.
    let mut guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = guard.as_ref() {
        return Ok(existing.clone());
    }
    let vault = SecretBox::open(SERVICE_NAME, storage_dir)?;
    log::info!(
        "[OfflineCache/Vault] Opened SecretBox backend={:?}",
        vault.backend_kind()
    );
    *guard = Some(vault.clone());
    Ok(vault)
}
