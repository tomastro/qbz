use std::collections::HashSet;

use url::Url;

use crate::model::{HandoffCandidate, LanJwtToken, RawHandoffRequest, RawLanJwtToken};

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_SESSION_ID_BYTES: usize = 256;
pub const MAX_ENDPOINT_BYTES: usize = 2 * 1024;
pub const MAX_JWT_BYTES: usize = 16 * 1024;
pub const MIN_TOKEN_LIFETIME_SECS: i64 = 60;

#[derive(Debug, Clone, Default)]
pub struct EndpointPolicy {
    api_hosts: HashSet<String>,
    qconnect_hosts: HashSet<String>,
}

impl EndpointPolicy {
    pub fn new(
        api_hosts: impl IntoIterator<Item = impl Into<String>>,
        qconnect_hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            api_hosts: normalize_hosts(api_hosts),
            qconnect_hosts: normalize_hosts(qconnect_hosts),
        }
    }

    pub fn from_trusted_endpoints(
        api_endpoint: &str,
        qconnect_endpoint: &str,
    ) -> Result<Self, ValidationError> {
        let api = endpoint_host(api_endpoint, "https")?;
        let qconnect = endpoint_host(qconnect_endpoint, "wss")?;
        Ok(Self::new([api], [qconnect]))
    }

    fn allows_api(&self, host: &str) -> bool {
        self.api_hosts.contains(&host.to_ascii_lowercase())
    }

    fn allows_qconnect(&self, host: &str) -> bool {
        self.qconnect_hosts.contains(&host.to_ascii_lowercase())
    }
}

fn normalize_hosts(hosts: impl IntoIterator<Item = impl Into<String>>) -> HashSet<String> {
    hosts
        .into_iter()
        .map(Into::into)
        .map(|host: String| host.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    InvalidShape,
    BecomeActiveRequired,
    SessionIdInvalid,
    TokenInvalid,
    TokenExpiring,
    EndpointInvalid,
    EndpointNotAllowed,
}

pub(crate) fn parse_and_validate(
    body: &[u8],
    policy: &EndpointPolicy,
    now_unix_secs: i64,
) -> Result<HandoffCandidate, ValidationError> {
    let mut raw: RawHandoffRequest =
        serde_json::from_slice(body).map_err(|_| ValidationError::InvalidShape)?;

    if !raw.become_active {
        return Err(ValidationError::BecomeActiveRequired);
    }
    if raw.session_id.is_empty()
        || raw.session_id.len() > MAX_SESSION_ID_BYTES
        || raw.session_id.trim() != raw.session_id
    {
        return Err(ValidationError::SessionIdInvalid);
    }

    let api = validate_token(&mut raw.jwt_api, "https", policy, now_unix_secs, true)?;
    let qconnect = validate_token(&mut raw.jwt_qconnect, "wss", policy, now_unix_secs, false)?;
    let session_id = std::mem::take(&mut raw.session_id);
    Ok(HandoffCandidate::new(session_id, api, qconnect, true))
}

fn validate_token(
    raw: &mut RawLanJwtToken,
    required_scheme: &str,
    policy: &EndpointPolicy,
    now_unix_secs: i64,
    api: bool,
) -> Result<LanJwtToken, ValidationError> {
    if raw.endpoint.is_empty()
        || raw.endpoint.len() > MAX_ENDPOINT_BYTES
        || raw.jwt.is_empty()
        || raw.jwt.len() > MAX_JWT_BYTES
    {
        return Err(ValidationError::TokenInvalid);
    }
    if raw.exp < now_unix_secs.saturating_add(MIN_TOKEN_LIFETIME_SECS) {
        return Err(ValidationError::TokenExpiring);
    }

    let url = Url::parse(&raw.endpoint).map_err(|_| ValidationError::EndpointInvalid)?;
    if url.scheme() != required_scheme
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ValidationError::EndpointInvalid);
    }
    let host = url
        .host_str()
        .ok_or(ValidationError::EndpointInvalid)?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let allowed = if api {
        policy.allows_api(&host)
    } else {
        policy.allows_qconnect(&host)
    };
    if !allowed {
        return Err(ValidationError::EndpointNotAllowed);
    }

    Ok(LanJwtToken::new(
        std::mem::take(&mut raw.endpoint),
        raw.exp,
        std::mem::take(&mut raw.jwt),
    ))
}

fn endpoint_host(endpoint: &str, required_scheme: &str) -> Result<String, ValidationError> {
    let url = Url::parse(endpoint).map_err(|_| ValidationError::EndpointInvalid)?;
    if url.scheme() != required_scheme
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ValidationError::EndpointInvalid);
    }
    url.host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .ok_or(ValidationError::EndpointInvalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_780_000_000;

    fn policy() -> EndpointPolicy {
        EndpointPolicy::new(["api.qobuz.test"], ["qws.qobuz.test"])
    }

    fn body(api_endpoint: &str, qws_endpoint: &str, exp: i64, active: bool) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "session_id": "controller-session",
            "jwt_api": { "endpoint": api_endpoint, "exp": exp, "jwt": "api-secret" },
            "jwt_qconnect": { "endpoint": qws_endpoint, "exp": exp, "jwt": "qws-secret" },
            "become_active": active
        }))
        .unwrap()
    }

    fn validation_error(body: &[u8]) -> ValidationError {
        match parse_and_validate(body, &policy(), NOW) {
            Ok(_) => panic!("handoff unexpectedly validated"),
            Err(error) => error,
        }
    }

    #[test]
    fn validates_exact_official_shape() {
        let candidate = parse_and_validate(
            &body(
                "https://api.qobuz.test/v1",
                "wss://qws.qobuz.test/ws",
                NOW + 600,
                true,
            ),
            &policy(),
            NOW,
        )
        .unwrap();

        assert_eq!(candidate.session_id(), "controller-session");
        assert_eq!(candidate.api_token().jwt(), "api-secret");
        assert_eq!(candidate.qconnect_token().jwt(), "qws-secret");
        let (session_id, api, qconnect, become_active) = candidate.into_parts();
        assert_eq!(session_id, "controller-session");
        assert_eq!(api.into_parts().2, "api-secret");
        assert_eq!(qconnect.into_parts().2, "qws-secret");
        assert!(become_active);
    }

    #[test]
    fn tolerates_future_fields_while_requiring_canonical_fields() {
        let future = serde_json::json!({
            "session_id": "controller-session",
            "jwt_api": {
                "endpoint": "https://api.qobuz.test/v1",
                "exp": NOW + 600,
                "jwt": "api-secret",
                "future_claim": "ignored"
            },
            "jwt_qconnect": {
                "endpoint": "wss://qws.qobuz.test/ws",
                "exp": NOW + 600,
                "jwt": "qws-secret",
                "future_claim": { "version": 2 }
            },
            "become_active": true,
            "future_capability": ["one", "two"]
        });

        let candidate =
            parse_and_validate(&serde_json::to_vec(&future).unwrap(), &policy(), NOW).unwrap();

        assert_eq!(candidate.session_id(), "controller-session");
        assert_eq!(candidate.api_token().jwt(), "api-secret");
        assert_eq!(candidate.qconnect_token().jwt(), "qws-secret");
    }

    #[test]
    fn rejects_aliases_expiry_and_untrusted_hosts() {
        let alias = serde_json::json!({
            "session_id": "controller-session",
            "jwt_api": { "endpoint": "https://api.qobuz.test", "exp": NOW + 600, "jwt": "a" },
            "jwt_qws": { "endpoint": "wss://qws.qobuz.test", "exp": NOW + 600, "jwt": "q" },
            "become_active": true
        });
        assert_eq!(
            validation_error(&serde_json::to_vec(&alias).unwrap()),
            ValidationError::InvalidShape
        );
        assert_eq!(
            validation_error(&body(
                "https://api.qobuz.test",
                "wss://qws.qobuz.test",
                NOW + 59,
                true,
            )),
            ValidationError::TokenExpiring
        );
        assert_eq!(
            validation_error(&body(
                "https://evil.test",
                "wss://qws.qobuz.test",
                NOW + 600,
                true,
            )),
            ValidationError::EndpointNotAllowed
        );
    }

    #[test]
    fn rejects_wrong_schemes_userinfo_and_inactive_handoff() {
        for api in [
            "http://api.qobuz.test",
            "https://api.qobuz.test:8443",
            "https://user@api.qobuz.test",
            "https://api.qobuz.test/#fragment",
        ] {
            assert_eq!(
                validation_error(&body(api, "wss://qws.qobuz.test", NOW + 600, true)),
                ValidationError::EndpointInvalid
            );
        }
        assert_eq!(
            validation_error(&body(
                "https://api.qobuz.test",
                "wss://qws.qobuz.test:8443",
                NOW + 600,
                true,
            )),
            ValidationError::EndpointInvalid
        );
        assert_eq!(
            validation_error(&body(
                "https://api.qobuz.test",
                "wss://qws.qobuz.test",
                NOW + 600,
                false,
            )),
            ValidationError::BecomeActiveRequired
        );
    }
}
