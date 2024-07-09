use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rustls::OwnedTrustAnchor;
use tokio_rustls::rustls::{Certificate, PrivateKey};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::TlsConnector;

use crate::CapybaraError;

#[inline]
fn load_keys(source: Source) -> Result<PrivateKey> {
    let keys = match source {
        Source::Content(key) => read_keys(key.as_bytes())?,
        Source::Path(path) => {
            let c = std::fs::read(path)?;
            read_keys(&c[..])?
        }
    };
    Ok(keys)
}

#[inline]
fn read_keys(b: &[u8]) -> Result<PrivateKey> {
    let mut r = BufReader::new(b);
    loop {
        match rustls_pemfile::read_one(&mut r)? {
            Some(rustls_pemfile::Item::RSAKey(key)) => {
                return Ok(PrivateKey(key));
            }
            Some(rustls_pemfile::Item::PKCS8Key(key)) => {
                return Ok(PrivateKey(key));
            }
            None => break,
            _ => (),
        }
    }
    bail!("no keys found")
}

#[inline]
fn read_certs(b: &[u8]) -> Result<Vec<Certificate>> {
    let mut r = BufReader::new(b);

    let mut certs = vec![];
    for next in rustls_pemfile::certs(&mut r)? {
        certs.push(Certificate(next));
    }

    Ok(certs)
}

#[inline]
fn load_certs(source: Source) -> Result<Vec<Certificate>> {
    let certs = match source {
        Source::Content(crt) => read_certs(crt.as_bytes())?,
        Source::Path(path) => {
            let c = std::fs::read(path)?;
            read_certs(&c[..])?
        }
    };
    Ok(certs)
}

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
        let Self { crt, key } = self;
        let certs = {
            let source = crt.ok_or_else(|| CapybaraError::InvalidTlsConfig("cert".into()))?;
            load_certs(source)?
        };
        let keys = {
            let source = key.ok_or_else(|| CapybaraError::InvalidTlsConfig("key".into()))?;
            load_keys(source)?
        };

        let config = rustls::ServerConfig::builder()
            .with_safe_defaults()
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

        if let Some(crt) = crt {
            let certs = load_certs(crt)?;

            for next in certs {
                root_cert_store.add(&next)?;
            }
        }

        root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|it| {
            OwnedTrustAnchor::from_subject_spki_name_constraints(
                it.subject,
                it.spki,
                it.name_constraints,
            )
        }));

        let config = ClientConfig::builder()
            .with_safe_defaults()
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
    use rustls::ServerName;
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
