use aes::cipher::{BlockDecryptMut, KeyIvInit, StreamCipher};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::error::CmafError;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

/// Strict hex decode for the app seed (HKDF IKM). Fail closed on odd length
/// or non-hex digits — never zero-fill bad nibbles.
fn hex_decode(hex: &str) -> Result<Vec<u8>, CmafError> {
    if hex.len() % 2 != 0 {
        return Err(CmafError::InvalidInfos(format!(
            "seed hex must have even length, got {}",
            hex.len()
        )));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| {
            CmafError::InvalidInfos(format!("invalid seed hex digit at offset {i}"))
        })?;
        out.push(byte);
    }
    Ok(out)
}

/// Derive the 16-byte session key from the session/start `infos` field.
///
/// `infos` format: `"salt_b64url.info_b64url"`
/// `seed` is the hex-encoded app seed used as IKM for HKDF (provided by caller).
pub fn derive_session_key(seed: &str, infos: &str) -> Result<[u8; 16], CmafError> {
    let parts: Vec<&str> = infos.split('.').collect();
    if parts.len() < 2 {
        return Err(CmafError::InvalidInfos(
            "session infos must have at least 2 dot-separated parts".into(),
        ));
    }

    let salt = URL_SAFE_NO_PAD.decode(parts[0])?;
    let info = URL_SAFE_NO_PAD.decode(parts[1])?;

    let ikm = hex_decode(seed)?;

    let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut okm = [0u8; 16];
    hk.expand(&info, &mut okm).map_err(|_| CmafError::HkdfExpand)?;

    Ok(okm)
}

/// Unwrap the per-track content key using the session key.
///
/// `key_str` format: `"qbz-1.wrapped_key_b64url.iv_b64url"`
pub fn unwrap_content_key(session_key: &[u8; 16], key_str: &str) -> Result<[u8; 16], CmafError> {
    let parts: Vec<&str> = key_str.split('.').collect();
    if parts.len() < 3 {
        return Err(CmafError::InvalidKey(
            "key string must have at least 3 dot-separated parts".into(),
        ));
    }

    let wrapped = URL_SAFE_NO_PAD.decode(parts[1])?;
    let iv = URL_SAFE_NO_PAD.decode(parts[2])?;

    if iv.len() != 16 {
        return Err(CmafError::InvalidKey(format!(
            "unwrap IV must be 16 bytes, got {}",
            iv.len()
        )));
    }

    let mut buf = wrapped.clone();
    let decrypted =
        Aes128CbcDec::new(session_key.into(), iv.as_slice().into())
            .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
            .map_err(|e| CmafError::AesDecrypt(format!("AES-CBC unwrap failed: {e}")))?;

    if decrypted.len() != 16 {
        return Err(CmafError::InvalidKey(format!(
            "unwrapped key must be 16 bytes, got {}",
            decrypted.len()
        )));
    }

    let mut key = [0u8; 16];
    key.copy_from_slice(decrypted);
    Ok(key)
}

/// Decrypt a FLAC frame in-place using AES-128-CTR.
///
/// `iv_8` = 8-byte IV from the segment UUID box entry, zero-padded to 16 bytes.
pub fn decrypt_frame(content_key: &[u8; 16], iv_8: &[u8; 8], data: &mut [u8]) {
    let mut nonce = [0u8; 16];
    nonce[..8].copy_from_slice(iv_8);
    Aes128Ctr::new(content_key.into(), &nonce.into()).apply_keystream(data);
}

/// Compute the MD5 request signature for Qobuz CMAF API calls.
///
/// Concatenates method + sorted key-value pairs + timestamp + seed,
/// then returns the lowercase hex MD5 digest.
/// `seed` is the app seed provided by the caller.
pub fn compute_request_sig(
    method: &str,
    args: &std::collections::BTreeMap<&str, String>,
    timestamp: &str,
    seed: &str,
) -> String {
    use md5::{Digest, Md5};

    let mut hasher = Md5::new();
    hasher.update(method.as_bytes());
    for (k, v) in args {
        hasher.update(k.as_bytes());
        hasher.update(v.as_bytes());
    }
    hasher.update(timestamp.as_bytes());
    hasher.update(seed.as_bytes());

    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SEED: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn test_compute_request_sig() {
        let mut args = std::collections::BTreeMap::new();
        args.insert("profile", "qbz-1".to_string());
        let sig = compute_request_sig("sessionstart", &args, "1775500000", TEST_SEED);
        assert_eq!(sig.len(), 32);
        let sig2 = compute_request_sig("sessionstart", &args, "1775500000", TEST_SEED);
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_decrypt_frame_roundtrip() {
        let key = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let iv = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let original = b"Hello FLAC frame data here!".to_vec();
        let mut data = original.clone();
        decrypt_frame(&key, &iv, &mut data);
        assert_ne!(data, original);
        decrypt_frame(&key, &iv, &mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn test_derive_session_key_invalid_infos() {
        let result = derive_session_key(TEST_SEED, "no_dot_here");
        assert!(result.is_err());
    }

    #[test]
    fn test_hex_decode_rejects_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn test_hex_decode_rejects_non_hex() {
        assert!(hex_decode("zz").is_err());
        assert!(hex_decode("0g").is_err());
    }

    #[test]
    fn test_hex_decode_empty_and_valid() {
        assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
        assert_eq!(hex_decode("00ff").unwrap(), vec![0x00, 0xff]);
        assert_eq!(hex_decode(TEST_SEED).unwrap().len(), 16);
    }

    #[test]
    fn test_derive_session_key_rejects_bad_seed_hex() {
        assert!(derive_session_key("not-hex!!", "YQ.YQ").is_err());
        assert!(derive_session_key("abc", "YQ.YQ").is_err());
    }

    #[test]
    fn test_unwrap_content_key_invalid_format() {
        let key = [0u8; 16];
        let result = unwrap_content_key(&key, "only.two");
        assert!(result.is_err());
    }
}
