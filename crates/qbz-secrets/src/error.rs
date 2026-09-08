use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("no backend available — keyring failed and KDF fallback could not initialize: {0}")]
    NoBackend(String),

    #[error("keyring error: {0}")]
    Keyring(String),

    #[error("cipher error: {0}")]
    Cipher(String),

    #[error("wrapped blob is malformed: {0}")]
    Malformed(String),

    #[error("wrapped blob was produced by a backend the current process cannot use")]
    BackendMismatch,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unexpected: {0}")]
    Other(String),
}
