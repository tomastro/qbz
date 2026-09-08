pub mod device_cap;
pub mod diagnostics;
pub mod memory_watchdog;
pub mod offline_mode;
pub mod graphics_autoconfig;
pub mod listen_log;
pub mod playback_context;
pub mod playback_driver;
pub mod qconnect_identity;
pub mod runtime;
pub mod scrobble_timing;
pub mod session_persist;
pub mod session_store;
pub mod subscription_lifecycle;
pub mod shell;
pub mod settings;
pub mod user_data;

/// Install the rustls process-level `CryptoProvider` (aws-lc-rs) exactly once.
///
/// Idempotent (a second call is a no-op); harmless if some other component
/// already installed a default (`install_default` then returns Err, ignored).
pub fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .is_err()
        {
            log::debug!("[app] rustls CryptoProvider already installed");
        }
    });
}
