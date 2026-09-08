//! Qobuz Connect cloud authentication.
//!
//! The official clients send `POST /qws/createToken` with exactly one form
//! field (`jwt=jwt_qws`). Their `user_auth_token_needed` and
//! `strong_auth_needed` values are interceptor controls and never cross the
//! HTTP boundary. Keeping the request here prevents frontend adapters from
//! drifting from that wire contract.

use serde::Deserialize;

use crate::client::QobuzClient;
use crate::endpoints::{self, paths};
use crate::error::{ApiError, Result};

const QCONNECT_TOKEN_FORM: [(&str, &str); 1] = [("jwt", "jwt_qws")];

/// A QWS credential returned by Qobuz.
///
/// Deliberately does not implement `Debug`: `jwt` is an in-memory secret and
/// must not leak through formatted errors or diagnostics.
#[derive(Clone, Deserialize)]
pub struct QwsAuthToken {
    endpoint: Option<String>,
    exp: i64,
    jwt: String,
}

impl QwsAuthToken {
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    pub const fn expires_at(&self) -> i64 {
        self.exp
    }

    pub fn jwt(&self) -> &str {
        &self.jwt
    }

    pub fn into_parts(self) -> (Option<String>, i64, String) {
        (self.endpoint, self.exp, self.jwt)
    }
}

#[derive(Deserialize)]
struct QwsAuthResponse {
    jwt_qws: Option<QwsAuthToken>,
}

impl QobuzClient {
    /// Mint the owner credential used by the native QConnect WebSocket
    /// transport. The response body is never included in an error because a
    /// successful body contains the JWT itself.
    pub async fn create_qconnect_token(&self) -> Result<QwsAuthToken> {
        let headers = self.authenticated_headers().await?;
        let response = self
            .http()?
            .post(endpoints::build_url(paths::QWS_CREATE_TOKEN))
            .headers(headers)
            .form(&QCONNECT_TOKEN_FORM)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(ApiError::ApiResponse(format!(
                "qws/createToken failed with HTTP {status}"
            )));
        }

        let payload: QwsAuthResponse = serde_json::from_str(&body)?;
        payload.jwt_qws.ok_or_else(|| {
            ApiError::ApiResponse("qws/createToken response missing jwt_qws".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::QCONNECT_TOKEN_FORM;

    #[test]
    fn create_token_form_matches_official_wire() {
        assert_eq!(QCONNECT_TOKEN_FORM, [("jwt", "jwt_qws")]);
    }
}
