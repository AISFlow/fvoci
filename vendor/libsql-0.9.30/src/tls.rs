//! Remote HTTPS connector on rustls 0.23.
//!
//! hyper-rustls 0.25 only builds against rustls 0.22, which pins
//! rustls-webpki 0.102. That 0.102 line has no patched release. Certificate
//! checks stay inside rustls: platform trust anchors, the default server
//! verifier, no custom verifier, and no certificate-revocation list. That is
//! the policy of hyper-rustls 0.25
//! `HttpsConnectorBuilder::new().with_native_roots().https_or_http().enable_http1()`,
//! including HTTP/1 with an empty ALPN list.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::uri::Scheme;
use hyper::client::connect::{Connected, Connection};
use hyper::service::Service;
use hyper::Uri;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// hyper 0.14 connector that speaks TLS with rustls 0.23.
#[derive(Clone)]
pub struct HttpsConnector<T> {
    http: T,
    tls: Arc<ClientConfig>,
}

impl<T> HttpsConnector<T> {
    /// Trust the platform certificate store and require rustls's default
    /// server verifier. Fails when the store cannot be read or yields no
    /// parsable root.
    pub fn with_native_roots(http: T) -> io::Result<Self> {
        let mut roots = RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs()?;
        let (valid, invalid) = roots.add_parsable_certificates(certs);
        if valid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no valid native root CA certificates found ({invalid} invalid)"),
            ));
        }
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            http,
            tls: Arc::new(config),
        })
    }
}

impl<T> Service<Uri> for HttpsConnector<T>
where
    T: Service<Uri>,
    T::Response: Connection + AsyncRead + AsyncWrite + Send + Unpin + 'static,
    T::Future: Send + 'static,
    T::Error: Into<BoxError>,
{
    type Response = MaybeHttpsStream<T::Response>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.http.poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        match dst.scheme() {
            Some(scheme) if scheme == &Scheme::HTTP => {
                let future = self.http.call(dst);
                return Box::pin(async move {
                    Ok(MaybeHttpsStream::Http(future.await.map_err(Into::into)?))
                });
            }
            Some(scheme) if scheme != &Scheme::HTTPS => {
                let message = format!("unsupported scheme {scheme}");
                return Box::pin(async move {
                    Err(io::Error::new(io::ErrorKind::Other, message).into())
                });
            }
            Some(_) => {}
            None => {
                return Box::pin(async move {
                    Err(io::Error::new(io::ErrorKind::Other, "missing scheme").into())
                });
            }
        }

        let cfg = self.tls.clone();
        let mut hostname = dst.host().unwrap_or_default();
        if let Some(trimmed) = hostname
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
        {
            hostname = trimmed;
        }
        let hostname = match ServerName::try_from(hostname) {
            Ok(name) => name.to_owned(),
            Err(_) => {
                return Box::pin(async move {
                    Err(io::Error::new(io::ErrorKind::Other, "invalid dnsname").into())
                });
            }
        };
        let connecting = self.http.call(dst);
        Box::pin(async move {
            let tcp = connecting.await.map_err(Into::into)?;
            let tls = TlsConnector::from(cfg)
                .connect(hostname, tcp)
                .await
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
            Ok(MaybeHttpsStream::Https(tls))
        })
    }
}

/// A stream that might be protected with TLS.
#[allow(clippy::large_enum_variant)]
pub enum MaybeHttpsStream<T> {
    /// A stream over plain text.
    Http(T),
    /// A stream protected with TLS.
    Https(TlsStream<T>),
}

impl<T: AsyncRead + AsyncWrite + Connection + Unpin> Connection for MaybeHttpsStream<T> {
    fn connected(&self) -> Connected {
        match self {
            Self::Http(stream) => stream.connected(),
            Self::Https(stream) => {
                let (tcp, tls) = stream.get_ref();
                if tls.alpn_protocol() == Some(b"h2") {
                    tcp.connected().negotiated_h2()
                } else {
                    tcp.connected()
                }
            }
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for MaybeHttpsStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match Pin::get_mut(self) {
            Self::Http(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Https(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for MaybeHttpsStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match Pin::get_mut(self) {
            Self::Http(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Https(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match Pin::get_mut(self) {
            Self::Http(stream) => Pin::new(stream).poll_flush(cx),
            Self::Https(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match Pin::get_mut(self) {
            Self::Http(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Https(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
