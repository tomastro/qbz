//! Library error types

use thiserror::Error;

#[derive(Error, Debug)]
pub enum LibraryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Metadata error: {0}")]
    Metadata(String),

    #[error("CUE parse error: {0}")]
    CueParse(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("{0}")]
    Other(String),
}
