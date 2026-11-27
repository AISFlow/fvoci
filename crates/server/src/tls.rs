//! Serving over TLS: `serve_https`.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use axum::serve::Listener;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::pem::PemObject as _;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::server::TlsStream;

/// The TLS settings of a listener.
pub(crate) type ServerConfig = Arc<rustls::ServerConfig>;

/// How long a client may take to complete the TLS handshake before the connection is
/// dropped, so that silent connections do not pile up.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long accepting pauses after an error that is not about one connection, such as
/// running out of file descriptors. The same pause the plain listener makes.
const ACCEPT_ERROR_PAUSE: Duration = Duration::from_secs(1);

/// The files that hold the server's certificate and private key, both in PEM format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    /// The certificate, followed by the certificates of its chain if any.
    pub certificate: PathBuf,
    /// The private key of the certificate.
    pub key: PathBuf,
}

impl Default for TlsFiles {
    /// Default certificate and key paths relative to the server's working directory.
    fn default() -> Self {
        Self {
            certificate: PathBuf::from("./cert/cert.pem"),
            key: PathBuf::from("./cert/key.pem"),
        }
    }
}

impl TlsFiles {
    /// The files that are not there, for the refusal to start without them.
    pub(crate) fn missing(&self) -> Vec<&std::path::Path> {
        [&self.certificate, &self.key]
            .into_iter()
            .filter(|path| !path.is_file())
            .map(PathBuf::as_path)
            .collect()
    }

    /// Reads the files into the settings of a TLS listener: TLS 1.2 and 1.3, no
    /// client certificates, HTTP/1.1.
    pub(crate) fn load(&self) -> anyhow::Result<ServerConfig> {
        let certificates: Vec<CertificateDer<'static>> =
            CertificateDer::pem_file_iter(&self.certificate)
                .and_then(Iterator::collect)
                .with_context(|| {
                    format!(
                        "reading the certificate file {}",
                        self.certificate.display()
                    )
                })?;
        if certificates.is_empty() {
            bail!(
                "the certificate file {} holds no certificate",
                self.certificate.display()
            );
        }
        let key = PrivateKeyDer::from_pem_file(&self.key)
            .with_context(|| format!("reading the key file {}", self.key.display()))?;

        // The provider is named because the dependency tree holds two, and rustls
        // refuses to pick one by itself then.
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("choosing the TLS versions")?
            .with_no_client_auth()
            .with_single_cert(certificates, key)
            .context("the certificate and the key cannot be used together")?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

/// A TCP listener whose connections speak TLS.
///
/// Handshakes run beside each other, so a slow client does not keep others waiting.
pub(crate) struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshakes: JoinSet<Option<(TlsStream<TcpStream>, SocketAddr)>>,
}

impl TlsListener {
    pub(crate) fn new(tcp: TcpListener, config: ServerConfig) -> Self {
        Self {
            tcp,
            acceptor: TlsAcceptor::from(config),
            handshakes: JoinSet::new(),
        }
    }

    fn start_handshake(&mut self, stream: TcpStream, peer: SocketAddr) {
        let acceptor = self.acceptor.clone();
        self.handshakes.spawn(async move {
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(stream)) => Some((stream, peer)),
                Ok(Err(error)) => {
                    tracing::debug!(%error, client = %peer.ip(), "TLS handshake failed");
                    None
                }
                Err(_) => {
                    tracing::debug!(client = %peer.ip(), "TLS handshake timed out");
                    None
                }
            }
        });
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                accepted = self.tcp.accept() => match accepted {
                    Ok((stream, peer)) => self.start_handshake(stream, peer),
                    Err(error) => pause_after_accept_error(&error).await,
                },
                Some(finished) = self.handshakes.join_next() => {
                    if let Ok(Some(connection)) = finished {
                        return connection;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// An error about one connection needs nothing; any other is logged and followed by a
/// pause, in the hope that whatever ran out comes back.
async fn pause_after_accept_error(error: &io::Error) {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!(%error, "accept error");
    tokio::time::sleep(ACCEPT_ERROR_PAUSE).await;
}
