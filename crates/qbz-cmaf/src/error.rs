#[derive(Debug, thiserror::Error)]
pub enum CmafError {
    #[error("Invalid infos format: {0}")]
    InvalidInfos(String),
    #[error("Invalid key format: {0}")]
    InvalidKey(String),
    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("HKDF expand error")]
    HkdfExpand,
    #[error("AES decrypt error: {0}")]
    AesDecrypt(String),
    #[error("CMAF parse error: {0}")]
    ParseError(String),
}
