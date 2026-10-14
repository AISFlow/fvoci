//! The one place FVOCI builds an HTTP client.
//!
//! TLS is rustls with the ring provider, trusting the bundled Mozilla roots
//! (`webpki-roots`), TLS 1.2 and 1.3, offering only `http/1.1` by ALPN:
//! what reqwest 0.12's `rustls-tls` feature set up. reqwest 0.13's `rustls`
//! feature would instead bring aws-lc-rs and the operating system verifier.
//! A second provider leaves rustls unable to pick one from crate features,
//! which sqlx, lettre and libsql rely on, and the verifier would change which
//! certificate authorities outbound requests trust. With
//! `rustls-no-provider`, a reqwest client built without this config panics
//! only while no process-wide rustls provider is installed. FVOCI installs
//! none, but sqlx (0.9.0, as 0.8.6) installs ring when it tries a PostgreSQL
//! connection with `sslmode=verify-ca`, even one whose certificate it then
//! refuses. From then on such a client builds and trusts the
//! operating system store, `SSL_CERT_FILE` or `SSL_CERT_DIR` instead of the
//! bundled roots. So every client starts here: clippy.toml rejects the
//! constructors it can name, and the `Default` ones are left to review.
//!
//! Each client gets its own config, so its own TLS session cache, as reqwest
//! built one per client before. Callers add their own redirect, proxy and
//! timeout policy.

use std::sync::Arc;

/// `reqwest::Client::builder()` with FVOCI's TLS configuration.
// clippy.toml disallows the bare constructors everywhere else.
#[allow(clippy::disallowed_methods)]
pub fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().tls_backend_preconfigured(tls_config(public_roots()))
}

/// The bundled Mozilla roots (`webpki-roots`) every client trusts.
fn public_roots() -> rustls::RootCertStore {
    rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// `reqwest::Client::new()` with FVOCI's TLS configuration and no further
/// policy.
///
/// # Panics
///
/// When the client cannot be built, as `reqwest::Client::new()` does.
pub fn client() -> reqwest::Client {
    builder().build().expect("HTTP client")
}

fn tls_config(roots: rustls::RootCertStore) -> rustls::ClientConfig {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(rustls::ALL_VERSIONS)
    .expect("ring supports TLS 1.2 and 1.3")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

#[cfg(test)]
mod tests {
    //! The shared client against a loopback rustls server whose leaf
    //! (`tests/fixtures/remote-tls/server.pem`, SAN `IP:127.0.0.1`) is issued
    //! by a test CA that no public root store holds, or whose leaf claims a
    //! public root as issuer without its key.
    use super::*;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::{Path, PathBuf};
    use std::thread::JoinHandle;

    const CHILD: &str = "FVOCI_HTTP_CLIENT_CHILD";

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/remote-tls")
            .join(name)
    }

    /// Runs in a fresh copy of this test binary, where nothing else has
    /// touched rustls: a reqwest client built without FVOCI's config panics
    /// there, and building one must not install a process-wide provider.
    #[test]
    fn builds_without_a_process_crypto_provider() {
        if std::env::var_os(CHILD).is_some() {
            assert!(rustls::crypto::CryptoProvider::get_default().is_none());
            drop(client());
            drop(builder().no_proxy().build().unwrap());
            assert!(rustls::crypto::CryptoProvider::get_default().is_none());
            return;
        }
        let selector = format!(
            "{}::builds_without_a_process_crypto_provider",
            module_path!().split_once("::").unwrap().1
        );
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &selector, "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success()
                && stdout.contains(&format!("test {selector} ... ok"))
                && stdout.contains("test result: ok. 1 passed"),
            "{selector} child {}:\n{stdout}\n{}",
            child.status,
            String::from_utf8_lossy(&child.stderr)
        );
    }

    /// Accepts one connection with the fixture leaf. Returns the request it
    /// decrypted and answered, or why the handshake ended.
    fn tls_server() -> (SocketAddr, JoinHandle<Result<String, String>>) {
        let chain = CertificateDer::pem_file_iter(fixture("server.pem"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_file(fixture("server.key")).unwrap();
        tls_server_with(chain, key)
    }

    /// [`tls_server`] presenting `chain` and `key`.
    fn tls_server_with(
        chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> (SocketAddr, JoinHandle<Result<String, String>>) {
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            // A second attempt is refused rather than left waiting.
            drop(listener);
            let mut tls = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            while tls.is_handshaking() {
                tls.complete_io(&mut tcp)
                    .map_err(|error| error.to_string())?;
            }
            let mut stream = rustls::StreamOwned::new(tls, tcp);
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .unwrap();
            stream.conn.send_close_notify();
            stream.flush().unwrap();
            Ok(String::from_utf8(head).unwrap())
        });
        (address, server)
    }

    async fn finish(server: JoinHandle<Result<String, String>>) -> Result<String, String> {
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap()
    }

    /// The rustls error behind a reqwest error. `io::Error::source` skips
    /// the error it wraps, so look inside each one.
    fn rustls_error<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a rustls::Error> {
        let mut next = Some(error);
        while let Some(error) = next {
            if let Some(found) = error.downcast_ref::<rustls::Error>() {
                return Some(found);
            }
            if let Some(inner) = error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
            {
                if let Some(found) = rustls_error(inner) {
                    return Some(found);
                }
            }
            next = error.source();
        }
        None
    }

    /// The subject of ISRG Root X1 (Let's Encrypt) as a trust anchor holds
    /// it, without the outer SEQUENCE: C=US, O=Internet Security Research
    /// Group, CN=ISRG Root X1.
    const ISRG_ROOT_X1: &[u8] = b"\x31\x0b\x30\x09\x06\x03\x55\x04\x06\x13\x02US\
        \x31\x29\x30\x27\x06\x03\x55\x04\x0a\x13\x20Internet Security Research Group\
        \x31\x15\x30\x13\x06\x03\x55\x04\x03\x13\x0cISRG Root X1";

    /// The refusal test below passes with no roots at all, so the store
    /// itself is checked here.
    #[test]
    fn trusts_the_bundled_mozilla_roots() {
        let roots = public_roots();
        assert!(!roots.is_empty());
        assert_eq!(roots.len(), webpki_roots::TLS_SERVER_ROOTS.len());
        assert!(roots
            .roots
            .iter()
            .any(|anchor| anchor.subject.as_ref() == ISRG_ROOT_X1));
    }

    /// Test only, trusted by nothing: a leaf (SAN `IP:127.0.0.1`, valid
    /// 2026-2126) whose issuer is ISRG Root X1 by name but which a throwaway
    /// RSA key signed, and its key. Made with OpenSSL 3.5:
    /// `openssl req -x509 -newkey rsa:2048` with `string_mask = nombstr` and
    /// the subject above for the issuer, then `openssl x509 -req -sha256`
    /// for an EC P-256 key with `serverAuth`.
    const FORGED_ISRG_LEAF: &str = "-----BEGIN CERTIFICATE-----
MIICqjCCAZKgAwIBAgIBAjANBgkqhkiG9w0BAQsFADBPMQswCQYDVQQGEwJVUzEp
MCcGA1UEChMgSW50ZXJuZXQgU2VjdXJpdHkgUmVzZWFyY2ggR3JvdXAxFTATBgNV
BAMTDElTUkcgUm9vdCBYMTAgFw0yNjAxMDEwMDAwMDBaGA8yMTI2MDEwMTAwMDAw
MFowITEfMB0GA1UEAwwWRlZPQ0kgZm9yZ2VkIElTUkcgbGVhZjBZMBMGByqGSM49
AgEGCCqGSM49AwEHA0IABF1bHLcVjZFFe72io1PROpecZyFnJUztOWWMB1XAkxGC
VGZuOy9Xblvet0QZln0tDWHUL+lZ64F3biMuLgxQeWijgYcwgYQwDAYDVR0TAQH/
BAIwADAOBgNVHQ8BAf8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwEwDwYDVR0R
BAgwBocEfwAAATAdBgNVHQ4EFgQUSbdJGK2mL7v//lJgtkfxfnsDjiowHwYDVR0j
BBgwFoAUQWPVm06xBj/221zrUMHUSHI+tocwDQYJKoZIhvcNAQELBQADggEBAECU
4/xIvVZWSm+fKSDcJQuamH6Bu3h/yLwUP2t9Lg4T6hEZPjhS87Vg2ueVswNiIrtp
Esf8O8JdSC8I+ADZGU/Iix4JD6JnigukMd3h9JNjljGgL2mtfgRvr7ru1mqm+eg+
n6Jms+v9OjSCV+DtB0SWrpBEbZJpAjyoXmQYUVmfnRFAcFTmaWpUevZ/98TaH+ix
l6ExWf/5uOweryqfcDYIFouvANVoAXDqP8ZjV+28NRa9GVbA3mnFfAqGcmhujF/E
S2pPvyd63BmsH8hr5onT2Y+Vj8Vey/cbkLnHCA8iV6dQgPErvTMtH9+Jtr2pIzWf
qjebJLJU/lC/uwqozR4=
-----END CERTIFICATE-----
";
    const FORGED_ISRG_LEAF_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg82WIuNSMU4DEbRuh
FiB49E40jYn3tgvhUx1/8SIMbUShRANCAARdWxy3FY2RRXu9oqNT0TqXnGchZyVM
7TlljAdVwJMRglRmbjsvV25b3rdEGZZ9LQ1h1C/pWeuBd24jLi4MUHlo
-----END PRIVATE KEY-----
";

    /// The root store `client()` really uses. With ISRG Root X1 in it, the
    /// forged leaf's issuer is found and its signature fails; without it,
    /// the issuer is unknown, as for the test CA below.
    #[tokio::test]
    async fn client_finds_isrg_root_x1_and_refuses_a_forged_signature() {
        let leaf = CertificateDer::from_pem_slice(FORGED_ISRG_LEAF.as_bytes()).unwrap();
        assert!(leaf
            .windows(ISRG_ROOT_X1.len())
            .any(|name| name == ISRG_ROOT_X1));
        let key = PrivateKeyDer::from_pem_slice(FORGED_ISRG_LEAF_KEY.as_bytes()).unwrap();
        let (address, server) = tls_server_with(vec![leaf], key);
        let error = client()
            .get(format!("https://{address}/"))
            .send()
            .await
            .expect_err("the leaf is not ISRG Root X1's");
        assert!(
            matches!(
                rustls_error(&error),
                Some(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::BadSignature
                ))
            ),
            "{error:?}"
        );
        finish(server).await.expect_err("handshake must fail");
    }

    #[tokio::test]
    async fn public_roots_refuse_a_private_ca() {
        let (address, server) = tls_server();
        let url = format!("https://{address}/");
        let error = client()
            .get(&url)
            .send()
            .await
            .expect_err("a test CA is not a public root");
        assert!(
            matches!(
                rustls_error(&error),
                Some(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::UnknownIssuer
                ))
            ),
            "{error:?}"
        );
        let refused = finish(server).await.expect_err("handshake must fail");
        assert!(refused.contains("UnknownCA"), "{refused}");

        // The same config trusting the test CA completes the request, so the
        // refusal above is the root store's.
        let (address, server) = tls_server();
        let mut roots = rustls::RootCertStore::empty();
        for ca in CertificateDer::pem_file_iter(fixture("ca.pem")).unwrap() {
            roots.add(ca.unwrap()).unwrap();
        }
        let trusting = builder()
            .tls_backend_preconfigured(tls_config(roots))
            .build()
            .unwrap();
        let response = trusting
            .get(format!("https://{address}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "ok");
        let request = finish(server).await.unwrap();
        assert!(request.starts_with("GET / HTTP/1.1\r\n"), "{request}");
    }
}
