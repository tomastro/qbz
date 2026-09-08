//! Errors for playlist import

use thiserror::Error;

// The Tauri original also had a `MissingCredentials` variant — dead code
// (zero call sites since the providers moved to the credential proxy);
// dropped in the extraction along with `ProviderCredentials`.
#[derive(Debug, Error)]
pub enum PlaylistImportError {
    #[error("Invalid playlist URL: {0}")]
    InvalidUrl(String),
    #[error("Provider not supported: {0}")]
    UnsupportedProvider(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Qobuz error: {0}")]
    Qobuz(String),

    // --- The 2.0.3 expansion's semantic variants (design §11.9) ------------
    //
    // These exist so the CONTROLLER can `match` and localize. The alternative
    // the design weighed and rejected was reusing `Parse(String)` with a
    // prefix and matching on the prefix — fragile in exactly the way a user
    // never sees until their language is not English.
    /// The bytes are not any playlist format this build reads.
    #[error("Unrecognized playlist format")]
    UnrecognizedFormat,
    /// An `.m3u8` that is an HLS STREAM manifest, not a playlist. Its own
    /// variant because the user's mistake is specific and so is the fix.
    #[error("HLS stream manifest, not a playlist")]
    HlsManifest,
    /// Parsed fine and yielded nothing usable.
    #[error("Playlist has no tracks")]
    EmptyPlaylist,
    /// Over `MAX_IMPORT_BYTES`. Refused BEFORE the parse, and on the app side
    /// before the READ (design §7.3) — a 2 GB file must never reach RAM.
    #[error("File too large to import")]
    FileTooLarge,
    /// Valid JSON with no track-shaped array in it. Fails loud rather than
    /// half-importing whatever array it found first.
    #[error("No track list recognized in this JSON")]
    JsonShapeUnrecognized,
}
