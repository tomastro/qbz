//! Device-bound secret wrapping for QBZ.
//!
//! This crate exists to give QBZ a single, uniform way to wrap small
//! secrets (AES keys, session tokens, OAuth refresh material) so they
//! can be persisted to disk without giving an attacker with filesystem
//! access everything they need to use those secrets on another machine.
//!
//! # Why
//!
//! The immediate consumer is the offline music cache. Qobuz streams each
//! track with CMAF, where a per-track AES key is derived at runtime from
//! session material. To make "save for offline" work we must store each
//! track's content key somewhere. Storing it plaintext in SQLite next to
//! the encrypted audio files would defeat the purpose — copying the DB +
//! files is enough to play them anywhere. So we wrap the key.
//!
//! # Design
//!
//! Two backends, same API:
//!
//! 1. **OS keyring** (preferred): the master AES-256 key lives inside the
//!    OS secure store — libsecret/gnome-keyring on Linux, Keychain on
//!    macOS, DPAPI on Windows. This is the "gold standard" device binding
//!    used by commercial music apps.
//! 2. **KDF fallback** (headless): when the OS keyring is not reachable
//!    (typical on Raspberry Pi / server / Docker), the master key is
//!    derived on the fly via HKDF-SHA256 over `machine-id` + a persistent
//!    per-install UUID + a constant salt. This is weaker than the keyring
//!    path because anyone with filesystem access to `machine-id` and the
//!    install directory can reconstruct the key, but it still defeats
//!    naive copy-paste attacks across machines and it lets the daemon
//!    variant of QBZ work without a desktop session behind it.
//!
//! Both backends produce and consume the same on-disk [`WrappedSecret`]
//! envelope, so the caller never needs to care which path was used.
//!
//! # Threat model
//!
//! What this protects against:
//!
//! - Copying the offline cache directory (or the whole SQLite DB) to
//!   another machine. On the new machine the keyring entry is missing
//!   (or machine-id differs), so the wrapped keys can't be unwrapped.
//! - Casual inspection of the DB. Keys are not recoverable by reading
//!   the wrapped blob alone.
//!
//! What this does **not** protect against:
//!
//! - A determined attacker on the same machine with shell access and
//!   the ability to invoke QBZ: they can always ask QBZ itself to
//!   decrypt, which is the right property (same threat model as every
//!   local DRM).
//! - Someone re-compiling QBZ with telemetry on the decrypted bytes —
//!   that's a source-modification attack, not a cryptographic one, and
//!   it's outside the scope of at-rest wrapping.
//!
//! # API
//!
//! ```rust,ignore
//! use qbz_secrets::SecretBox;
//!
//! // Open once per app, at startup. service_name scopes the key inside
//! // the OS keyring and is part of the HKDF salt for the fallback path.
//! let vault = SecretBox::open("qbz", storage_dir).await?;
//!
//! // Wrap a content key before persisting to SQLite
//! let wrapped: Vec<u8> = vault.wrap(&content_key_bytes)?;
//!
//! // Unwrap when reading it back
//! let original: Vec<u8> = vault.unwrap(&wrapped)?;
//! ```

mod backend;
mod cipher;
mod envelope;
mod error;
mod install_id;

pub use backend::{Backend, BackendKind};
pub use envelope::WrappedSecret;
pub use error::SecretError;

use std::path::Path;

/// Handle to the secret storage. Cheap to clone (reference-counted
/// internally via `Arc` inside the backend).
#[derive(Clone)]
pub struct SecretBox {
    backend: std::sync::Arc<Backend>,
}

impl SecretBox {
    /// Open the secret store. Tries the OS keyring first; if that fails
    /// for any reason (headless, missing libsecret, keyring locked,
    /// user denied access), falls back transparently to the HKDF path.
    ///
    /// `service_name` scopes the key inside the OS keyring — use a
    /// constant per app. `storage_dir` is where the install UUID (salt
    /// for the KDF fallback) lives; it must be writable and persistent
    /// across app restarts.
    pub fn open(service_name: &str, storage_dir: &Path) -> Result<Self, SecretError> {
        let backend = Backend::new(service_name, storage_dir)?;
        Ok(Self {
            backend: std::sync::Arc::new(backend),
        })
    }

    /// Construct directly from a provided backend, useful for tests.
    #[doc(hidden)]
    pub fn from_backend(backend: Backend) -> Self {
        Self {
            backend: std::sync::Arc::new(backend),
        }
    }

    /// Wrap a secret for at-rest storage. The returned bytes are
    /// self-describing (include backend marker, nonce, ciphertext+tag)
    /// so [`unwrap`](Self::unwrap) on the same machine can round-trip.
    pub fn wrap(&self, plaintext: &[u8]) -> Result<Vec<u8>, SecretError> {
        self.backend.wrap(plaintext)
    }

    /// Unwrap previously [`wrap`](Self::wrap)-produced bytes.
    pub fn unwrap(&self, wrapped: &[u8]) -> Result<Vec<u8>, SecretError> {
        self.backend.unwrap(wrapped)
    }

    /// Which backend is actually in use. Exposed for diagnostics /
    /// settings UI ("Offline cache secured by OS keyring" vs "…by
    /// device-derived key").
    pub fn backend_kind(&self) -> BackendKind {
        self.backend.kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_vault() -> (SecretBox, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        // Pinned to the KDF path rather than letting `open` pick. The old
        // comment here claimed the sandboxed tempdir guaranteed a fresh state
        // and so exercised the fallback by design; that was never true, because
        // `try_open_keyring` ignores `storage_dir` entirely and looks the entry
        // up by service name alone. Once this crate asks for real keyring
        // backends, these four tests wrote a genuine login-Keychain item, and
        // every subsequent run built a new test binary that macOS then blocked
        // on an authorization dialog: four tests hanging forever, on every dev
        // machine, from the second run onwards.
        //
        // `survives_a_restart.rs` is where the real backend belongs, because
        // persistence across processes is what it is actually testing.
        let backend =
            Backend::kdf_only("qbz-secrets-test-harness", dir.path()).expect("kdf backend");
        (SecretBox::from_backend(backend), dir)
    }

    #[test]
    fn roundtrip_small_secret() {
        let (vault, _dir) = test_vault();
        let payload = b"hello, secret";
        let wrapped = vault.wrap(payload).expect("wrap");
        let unwrapped = vault.unwrap(&wrapped).expect("unwrap");
        assert_eq!(unwrapped, payload);
    }

    #[test]
    fn roundtrip_16_byte_content_key() {
        // The exact shape of a CMAF content key — the primary use case.
        let (vault, _dir) = test_vault();
        let key = [0x42u8; 16];
        let wrapped = vault.wrap(&key).expect("wrap");
        let unwrapped = vault.unwrap(&wrapped).expect("unwrap");
        assert_eq!(unwrapped, &key[..]);
    }

    #[test]
    fn tampering_is_detected() {
        let (vault, _dir) = test_vault();
        let mut wrapped = vault.wrap(b"important data").expect("wrap");
        // Flip one bit in the ciphertext region (past the 14-byte header)
        wrapped[20] ^= 0x01;
        let result = vault.unwrap(&wrapped);
        assert!(result.is_err(), "GCM tag must detect tampering");
    }

    #[test]
    fn two_wraps_of_same_plaintext_differ() {
        // Nonce is random per wrap — two calls must produce distinct ciphertext
        let (vault, _dir) = test_vault();
        let a = vault.wrap(b"same input").expect("wrap");
        let b = vault.wrap(b"same input").expect("wrap");
        assert_ne!(a, b);
    }
}
