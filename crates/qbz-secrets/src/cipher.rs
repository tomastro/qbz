//! AES-256-GCM authenticated encryption primitive shared by both backends.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngExt;

use crate::envelope::{WrappedSecret, NONCE_LEN};
use crate::error::SecretError;

pub(crate) fn wrap_with_key(
    key: &[u8; 32],
    backend_marker: u8,
    plaintext: &[u8],
) -> Result<Vec<u8>, SecretError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| SecretError::Cipher(e.to_string()))?;

    Ok(WrappedSecret::build(backend_marker, &nonce_bytes, &ciphertext))
}

pub(crate) fn unwrap_with_key(
    key: &[u8; 32],
    expected_marker: u8,
    wrapped: &[u8],
) -> Result<Vec<u8>, SecretError> {
    let parsed = WrappedSecret::parse(wrapped)?;
    if parsed.backend_marker != expected_marker {
        return Err(SecretError::BackendMismatch);
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(parsed.nonce);
    cipher
        .decrypt(nonce, parsed.ciphertext)
        .map_err(|e| SecretError::Cipher(e.to_string()))
}
