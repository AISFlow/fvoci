# FVOCI local libsql 0.9.30 patch

This is the maintained crates.io libsql 0.9.30 source at upstream VCS
`0653c5788d77ef16a97c56ff3e9fdc11717a72d9`, directory `libsql`.
It is a local FVOCI patch, not an upstream accepted release. See
`PROVENANCE.json` and `UPSTREAM-FILES.json` for immutable original inputs.

Original crate SHA-256:
`30fe980ac5693ed1f3db490559fb578885e913a018df64af8a1a46e1959a78df`.
That checksum identifies the unmodified archive, not this patched source.
All 70 archive files are retained. `src/errors.rs` is still the borrowed
Hrana code accessor described below. A second local change drops
`hyper-rustls` 0.25 (rustls 0.22 / rustls-webpki 0.102, which has no patched
release) and connects the existing remote client with rustls 0.23, the same
major already used by reqwest. See the TLS section. Changed upstream files
are `Cargo.toml`, `src/database.rs`, `src/lib.rs`, and `examples/flutter.rs`.
`src/tls.rs` is new. The Hrana accessor change is still only `src/errors.rs`: 23 inserted production lines add a borrowed
`Error::hrana_error_code` accessor under the existing `hrana` feature, and
135 inserted test lines supply five focused controls. Existing error variants,
formatting, conversions, transport, SQL and stream settlement are unchanged.
The SDK keeps responsibility for its private Hrana statement error types.
FVOCI compares the exposed machine code, never message text or HTTP JSON.
A code does not prove commit, rollback, or server Close acknowledgment.

`LICENSE.md` is the full 1069-byte MIT text recovered by ROOT from the same
pinned upstream VCS root; the published archive omitted that file. Its SHA-256
is `cecf589818a56c2da8a0a6cc82c8a215c9770988c033b6016c2b74bf79f38152`.
The crate version stays 0.9.30. Feature names are unchanged. The `tls` feature
now depends on rustls 0.23.45, tokio-rustls 0.26.5, and rustls-native-certs
0.7.3 (ring, no aws-lc-rs) instead of hyper-rustls 0.25. `Cargo.toml.orig`,
the vendored upstream `Cargo.lock`, and dev-dependencies are untouched. The
root `Cargo.lock` is the resolution fvoci-server builds.

Source adoption only: SDK compilation and five Rust controls, actual remote
FK failure and original-stream cleanup are NOTRUN. ROOT owns product patch
wiring, lock qualification, build/image notices, shared migration helper
integration, and separately allocated runtime acceptance.

## TLS connector

`Builder::new_remote(...).build()` with no custom connector calls
`database::connector()`. That function used to build
`hyper_rustls::HttpsConnector` with native roots, HTTP or HTTPS, and HTTP/1
(no ALPN list). `src/tls.rs` keeps that policy on rustls 0.23 and the ring
provider: platform roots via rustls-native-certs, rustls's default server
verifier, no client certificate, no revocation lists. Plain `http` is still
forwarded without TLS. The `tls` Cargo feature no longer enables the
`webpki-roots` set; production already called `with_native_roots()`, not
`with_webpki_roots()`. `examples/flutter.rs` now uses that same default
connector.
