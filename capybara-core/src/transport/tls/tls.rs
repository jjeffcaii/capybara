use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use coarsetime::Instant;
use once_cell::sync::Lazy;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::TlsConnector;

use crate::CapybaraError;

pub static DEFAULT_CONNECTOR: Lazy<TlsConnector> =
    Lazy::new(|| TlsConnectorBuilder::default().build().unwrap());

enum Source<'a> {
    Content(&'a str),
    Path(PathBuf),
}

#[derive(Default)]
pub struct TlsAcceptorBuilder<'a> {
    crt: Option<Source<'a>>,
    key: Option<Source<'a>>,
}

impl<'a> TlsAcceptorBuilder<'a> {
    pub fn new() -> Self {
        TlsAcceptorBuilder::default()
    }

    pub fn cert(mut self, cert: &'a str) -> Self {
        self.crt.replace(Source::Content(cert));
        self
    }

    pub fn key(mut self, key: &'a str) -> Self {
        self.key.replace(Source::Content(key));
        self
    }

    pub fn cert_path(mut self, path: PathBuf) -> Self {
        self.crt.replace(Source::Path(path));
        self
    }

    pub fn key_path(mut self, path: PathBuf) -> Self {
        self.key.replace(Source::Path(path));
        self
    }

    pub fn build(self) -> Result<TlsAcceptor> {
        use rustls::pki_types::CertificateDer;

        let Self { crt, key } = self;
        let certs = {
            let source = crt.ok_or_else(|| CapybaraError::InvalidTlsConfig("cert".into()))?;

            match source {
                Source::Content(content) => {
                    vec![CertificateDer::from_pem_slice(content.as_bytes())?]
                }
                Source::Path(path) => {
                    CertificateDer::pem_file_iter(path)?.collect::<Result<Vec<_>, _>>()?
                }
            }
        };
        let keys = {
            let source = key.ok_or_else(|| CapybaraError::InvalidTlsConfig("key".into()))?;
            match source {
                Source::Content(content) => PrivateKeyDer::from_pem_slice(content.as_bytes())?,
                Source::Path(path) => PrivateKeyDer::from_pem_file(path)?,
            }
        };

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, keys)
            .map_err(|err| CapybaraError::MalformedTlsConfig(err.into()))?;

        info!("create tls server configuration ok");

        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

#[derive(Default)]
pub struct TlsConnectorBuilder<'a> {
    crt: Option<Source<'a>>,
    key: Option<Source<'a>>,
}

impl<'a> TlsConnectorBuilder<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cert(mut self, cert: &'a str) -> Self {
        self.crt.replace(Source::Content(cert));
        self
    }

    pub fn key(mut self, key: &'a str) -> Self {
        self.key.replace(Source::Content(key));
        self
    }

    pub fn cert_path(mut self, path: PathBuf) -> Self {
        self.crt.replace(Source::Path(path));
        self
    }

    pub fn key_path(mut self, path: PathBuf) -> Self {
        self.key.replace(Source::Path(path));
        self
    }

    pub fn build(self) -> Result<TlsConnector> {
        let Self { crt, key } = self;

        let mut root_cert_store = RootCertStore::empty();

        // add system ca
        root_cert_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        // add custom ca
        if let Some(crt) = crt {
            match crt {
                Source::Content(content) => {
                    root_cert_store.add(CertificateDer::from_pem_slice(content.as_bytes())?)?;
                }
                Source::Path(path) => {
                    for cert in CertificateDer::pem_file_iter(path)? {
                        root_cert_store.add(cert?)?;
                    }
                }
            };
        }

        let config = ClientConfig::builder()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(config));
        Ok(connector)
    }
}

#[cfg(test)]
mod tls_tests {
    use std::net::SocketAddr;

    use bytes::BytesMut;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use crate::resolver::DEFAULT_RESOLVER;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_tls_connect() -> Result<()> {
        init();

        let ip = DEFAULT_RESOLVER.resolve_one("httpbin.org").await?;

        let c = TlsConnectorBuilder::default().build()?;

        let stream = TcpStream::connect(SocketAddr::new(ip, 443)).await?;

        let server_name = ServerName::try_from("httpbin.org")?;

        let mut connection = c.connect(server_name, stream).await?;

        connection
            .write_all(&b"GET /ip HTTP/1.1\r\nHost: httpbin.org\r\nAccept: *\r\n\r\n"[..])
            .await?;

        let mut b = BytesMut::with_capacity(4096);

        connection.read_buf(&mut b).await?;

        info!("response: {:?}", b.freeze());

        Ok(())
    }
}
