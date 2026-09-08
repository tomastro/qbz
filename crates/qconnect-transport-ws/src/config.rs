use std::fmt;

use zeroize::Zeroize;

/// WebSocket transport configuration.
///
/// The delegated endpoint can itself contain sensitive routing material, so
/// both it and `jwt_qws` are redacted from `Debug` and overwritten on drop.
#[derive(Clone)]
pub struct WsTransportConfig {
    pub endpoint_url: String,
    pub jwt_qws: Option<String>,
    /// When `true`, a connect attempt with `jwt_qws == None` is a hard
    /// credential error instead of silently skipping the AUTHENTICATE frame
    /// (gap #12). Defaults to `false` so the InMemory / test transport path
    /// keeps working without a JWT.
    pub require_jwt: bool,
    pub reconnect_backoff_ms: u64,
    pub reconnect_backoff_max_ms: u64,
    /// Maximum number of consecutive reconnect attempts before the transport
    /// gives up and shuts down. The counter resets only when a session-level
    /// join is confirmed (`SRVR_CTRL_SESSION_STATE` for controllers or
    /// targeted `SRVR_RNDR_SET_ACTIVE=true` for delegated renderers), not when
    /// the WS / TCP connection succeeds.
    ///
    /// `None` means unlimited (legacy behavior, retained for tests).
    pub reconnect_max_attempts: Option<u32>,
    /// When `> 0`, reaching `Exhausted` no longer terminates the transport
    /// loop: it idles this long (shutdown-cancellable), resets the attempt
    /// counter / backoff to base, and retries instead of giving up (gap #7).
    /// Default `0` preserves the legacy terminate-on-exhausted behavior used
    /// by tests; the real config sets 60s.
    pub reconnect_idle_retry_ms: u64,
    pub connect_timeout_ms: u64,
    pub keepalive_interval_ms: u64,
    pub auto_subscribe: bool,
    pub subscribe_channels: Vec<Vec<u8>>,
    pub qcloud_proto: u32,
}

impl WsTransportConfig {
    /// Overwrite and remove delegated QWS routing material immediately.
    ///
    /// This is also the single cleanup path used by `Drop`, which keeps the
    /// behavior directly testable without inspecting freed memory.
    fn clear_sensitive(&mut self) {
        self.endpoint_url.zeroize();
        if let Some(mut jwt_qws) = self.jwt_qws.take() {
            jwt_qws.zeroize();
        }
    }
}

impl fmt::Debug for WsTransportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsTransportConfig")
            .field("endpoint_url", &"[REDACTED]")
            .field("jwt_qws", &self.jwt_qws.as_ref().map(|_| "[REDACTED]"))
            .field("require_jwt", &self.require_jwt)
            .field("reconnect_backoff_ms", &self.reconnect_backoff_ms)
            .field("reconnect_backoff_max_ms", &self.reconnect_backoff_max_ms)
            .field("reconnect_max_attempts", &self.reconnect_max_attempts)
            .field("reconnect_idle_retry_ms", &self.reconnect_idle_retry_ms)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("keepalive_interval_ms", &self.keepalive_interval_ms)
            .field("auto_subscribe", &self.auto_subscribe)
            .field("subscribe_channels", &self.subscribe_channels)
            .field("qcloud_proto", &self.qcloud_proto)
            .finish()
    }
}

impl Drop for WsTransportConfig {
    fn drop(&mut self) {
        self.clear_sensitive();
    }
}

impl Default for WsTransportConfig {
    fn default() -> Self {
        Self {
            endpoint_url: String::new(),
            jwt_qws: None,
            require_jwt: false,
            reconnect_backoff_ms: 2_000,
            reconnect_backoff_max_ms: 30_000,
            reconnect_max_attempts: Some(10),
            reconnect_idle_retry_ms: 0,
            connect_timeout_ms: 10_000,
            keepalive_interval_ms: 30_000,
            auto_subscribe: true,
            subscribe_channels: Vec::new(),
            qcloud_proto: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_JWT: &str = "header.payload.secret-signature";
    const TEST_ENDPOINT: &str = "wss://qws.example.test/ws?route=secret";

    #[test]
    fn debug_redacts_present_jwt() {
        let mut config = WsTransportConfig::default();
        config.endpoint_url = TEST_ENDPOINT.to_string();
        config.jwt_qws = Some(TEST_JWT.to_string());

        let debug = format!("{config:?}");
        assert!(!debug.contains(TEST_JWT));
        assert!(!debug.contains(TEST_ENDPOINT));
        assert!(debug.contains("endpoint_url: \"[REDACTED]\""));
        assert!(debug.contains("jwt_qws: Some(\"[REDACTED]\")"));
    }

    #[test]
    fn cleanup_path_removes_jwt() {
        let mut config = WsTransportConfig::default();
        config.endpoint_url = TEST_ENDPOINT.to_string();
        config.jwt_qws = Some(TEST_JWT.to_string());

        config.clear_sensitive();

        assert!(config.endpoint_url.is_empty());
        assert!(config.jwt_qws.is_none());
    }
}
