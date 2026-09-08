//! On-disk envelope format for wrapped secrets.
//!
//! Layout (bytes):
//!
//! ```text
//! [0]       version (u8)       — currently always 1
//! [1]       backend marker     — 0 = keyring, 1 = kdf-fallback
//! [2..14]   nonce (12 bytes)   — random per wrap
//! [14..]    ciphertext+tag     — AES-256-GCM output
//! ```
//!
//! The backend marker is informational so we can refuse to unwrap a blob
//! that was produced by a backend the current process cannot reach (e.g.
//! a keyring-wrapped blob on a machine where the keyring is gone).

use crate::error::SecretError;

pub(crate) const ENVELOPE_VERSION: u8 = 1;
pub(crate) const HEADER_LEN: usize = 2 + 12; // version + backend + nonce
pub(crate) const NONCE_LEN: usize = 12;

/// Internal view of a wrapped secret blob. `WrappedSecret` is the user-
/// facing type re-exported from `lib.rs` for ergonomics in diagnostics.
#[derive(Debug)]
pub struct WrappedSecret<'a> {
    pub version: u8,
    pub backend_marker: u8,
    pub nonce: &'a [u8],
    pub ciphertext: &'a [u8],
}

impl<'a> WrappedSecret<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, SecretError> {
        if bytes.len() < HEADER_LEN + 16 /* GCM tag */ {
            return Err(SecretError::Malformed(format!(
                "wrapped blob too short ({} bytes)",
                bytes.len()
            )));
        }
        let version = bytes[0];
        if version != ENVELOPE_VERSION {
            return Err(SecretError::Malformed(format!(
                "unsupported envelope version {}",
                version
            )));
        }
        Ok(Self {
            version,
            backend_marker: bytes[1],
            nonce: &bytes[2..HEADER_LEN],
            ciphertext: &bytes[HEADER_LEN..],
        })
    }

    pub fn build(backend_marker: u8, nonce: &[u8], ciphertext: &[u8]) -> Vec<u8> {
        assert_eq!(nonce.len(), NONCE_LEN, "nonce must be 12 bytes");
        let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        out.push(ENVELOPE_VERSION);
        out.push(backend_marker);
        out.extend_from_slice(nonce);
        out.extend_from_slice(ciphertext);
        out
    }
}
