# QBZ local patch of rust_cast 0.21.0

This is a verbatim copy of rust_cast 0.21.0 from crates.io with one minimal
fix applied. Keep the delta as small as possible and rebase it when bumping
the upstream version.

## Problem

Connecting to a Cast device fails during the TLS handshake with:

```
Connection error: invalid peer certificate: Other(OtherError(UnsupportedCertVersion))
```

Many Cast devices (notably AV receivers with Chromecast built-in) serve a
self-signed X.509 **version 1** certificate on port 8009. rust_cast's
`connect_without_host_verification` uses the `NoCertificateVerification`
verifier, which accepts any certificate in `verify_server_cert`, but its
`verify_tls12_signature`/`verify_tls13_signature` delegate to the rustls
helpers `rustls::crypto::verify_tls12_signature`/`verify_tls13_signature`.
Those helpers parse the end-entity certificate with webpki to extract the
public key, and webpki only supports X.509 v3 certificates, so the handshake
aborts for v1 device certificates. This has affected every rustls-based
rust_cast release (0.19+) and reproduces with rustls-webpki 0.102.x and
0.103.x alike; only devices that happen to serve v3 certificates can connect.

References: https://github.com/rustls/rustls/issues/1298
            https://github.com/rustls/webpki/issues/29

## Patch

In `src/lib.rs`, `NoCertificateVerification::verify_tls12_signature` and
`verify_tls13_signature` return `HandshakeSignatureValid::assertion()`
instead of delegating to the rustls helpers. This matches the verifier's
documented contract (no certificate validation at all) and the pattern used
by the rustls `danger` verifier examples. `supported_verify_schemes` still
delegates to the aws-lc-rs provider, and `verify_server_cert` is unchanged.

Verification against a real device serving a v1 certificate (TLS 1.2,
ECDHE-RSA-CHACHA20-POLY1305): the handshake and the CASTV2 receiver channel
now establish; the unpatched crate fails at the first channel write with the
error above.

## Upstream

The same fix is being proposed upstream (https://github.com/azasypkin/rust-cast).
Once a rust_cast release includes it, remove this directory and the
`[patch.crates-io]` entry in `crates/Cargo.toml`.
