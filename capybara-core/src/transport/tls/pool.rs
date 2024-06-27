use std::net::SocketAddr;
use std::result::Result as StdResult;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use deadpool::managed::{Metrics, RecycleError, RecycleResult};
use deadpool::{managed, Runtime};
use rustls::ServerName;
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::cachestr::Cachestr;
use crate::resolver::Resolver;
use crate::transport::Address;
use crate::transport::{tcp, TlsConnectorBuilder};
use crate::{resolver, CapybaraError};

pub type TlsStream<T> = tokio_rustls::client::TlsStream<T>;

pub(crate) type Pool = managed::Pool<Manager>;

pub(crate) struct TlsStreamPoolBuilder {
    addr: Address,
    max_size: usize,
    timeout: Option<Duration>,
    buff_size: usize,
    idle_time: Option<Duration>,
    resolver: Option<Arc<dyn Resolver>>,
    sni: Option<ServerName>,
}

impl TlsStreamPoolBuilder {
    pub(crate) const BUFF_SIZE: usize = 8192;
    pub(crate) const MAX_SIZE: usize = 128;

    pub fn with_addr(addr: SocketAddr) -> Self {
        Self::new(Address::Direct(addr))
    }

    pub fn with_domain<D>(domain: D, port: u16) -> Self
    where
        D: AsRef<str>,
    {
        let domain = Cachestr::from(domain.as_ref());
        Self::new(Address::Domain(domain, port))
    }

    #[inline(always)]
    fn new(addr: Address) -> Self {
        Self {
            addr,
            timeout: None,
            buff_size: TlsStreamPoolBuilder::BUFF_SIZE,
            idle_time: None,
            max_size: TlsStreamPoolBuilder::MAX_SIZE,
            resolver: None,
            sni: None,
        }
    }

    pub fn sni(mut self, server_name: ServerName) -> Self {
        self.sni.replace(server_name);
        self
    }

    pub fn max_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout.replace(timeout);
        self
    }

    pub fn buff_size(mut self, buff_size: usize) -> Self {
        self.buff_size = buff_size;
        self
    }

    pub fn idle_time(mut self, lifetime: Duration) -> Self {
        self.idle_time.replace(lifetime);
        self
    }

    pub fn resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver.replace(resolver);
        self
    }

    pub async fn build(self, closer: Arc<Notify>) -> Result<Pool> {
        let Self {
            addr,
            max_size,
            timeout,
            buff_size,
            idle_time,
            resolver,
            sni,
        } = self;

        let resolver: Arc<dyn Resolver> =
            resolver.unwrap_or_else(|| Clone::clone(&resolver::DEFAULT_RESOLVER));

        let sni = match sni {
            None => match &addr {
                Address::Direct(addr) => ServerName::IpAddress(addr.ip()),
                Address::Domain(domain, _) => {
                    let domain = domain.as_ref();
                    ServerName::try_from(domain).map_err(|e| {
                        error!("cannot generate sni from '{}': {}", domain, e);
                        CapybaraError::InvalidTlsSni(domain.to_string().into())
                    })?
                }
            },
            Some(sni) => sni,
        };

        let mgr = Manager {
            timeout,
            addr: Clone::clone(&addr),
            resolver,
            buff_size,
            sni,
        };

        let pool = Pool::builder(mgr)
            .wait_timeout(timeout)
            .max_size(max_size)
            .runtime(Runtime::Tokio1)
            .build()?;

        info!("initialize tcp conn pool of {}", &addr);

        if let Some(age) = idle_time {
            // clone the original pool
            let pool = Clone::clone(&pool);
            tokio::spawn(async move {
                // half of max-age, but at least 5s
                let interval = Duration::max(Duration::from_secs(5), age / 2);

                // init counters
                let alive_cnt = Arc::new(AtomicU64::new(0));
                let evicted_cnt = Arc::new(AtomicU64::new(0));

                // anti duplicated log
                let mut prev = (0, 0);

                // begin retain timer...
                loop {
                    tokio::select! {
                        _ = closer.notified() => {
                            info!("the idle checker for connection pool '{}' is stopped", addr);
                            break;
                        }
                        _ = tokio::time::sleep(interval) => {
                            let alive_cnt2 = Clone::clone(&alive_cnt);
                            let evicted_cnt2 = Clone::clone(&evicted_cnt);
                            pool.retain(move |c, metrics| {
                                // check max idle time
                                if metrics.last_used() > age {
                                    if log_enabled!(log::Level::Debug) {
                                        let (c, _) = c.get_ref();
                                        debug!("evict idle connection: {:?}", c.local_addr());
                                    }
                                    evicted_cnt2.fetch_add(1, Ordering::SeqCst);
                                    return false;
                                }

                                alive_cnt2.fetch_add(1, Ordering::SeqCst);

                                true
                            });

                            let next = (
                                evicted_cnt.load(Ordering::SeqCst),
                                alive_cnt.load(Ordering::SeqCst),
                            );

                            if prev != next {
                                info!("scale tcp conn pool of {}: evicted={}, idle={}", addr, next.0, next.1);
                                prev = next;
                            }

                            // reset counters
                            evicted_cnt.store(0, Ordering::SeqCst);
                            alive_cnt.store(0, Ordering::SeqCst);
                        }
                    }
                }
            });
        }

        Ok(pool)
    }
}

#[inline]
fn is_health(stream: &TlsStream<TcpStream>) -> crate::Result<()> {
    let (c, _) = stream.get_ref();
    tcp::is_health(c)
}

pub(crate) struct Manager {
    addr: Address,
    resolver: Arc<dyn Resolver>,
    buff_size: usize,
    timeout: Option<Duration>,
    sni: ServerName,
}

impl managed::Manager for Manager {
    type Type = TlsStream<TcpStream>;
    type Error = CapybaraError;

    async fn create(&self) -> StdResult<Self::Type, Self::Error> {
        let addr = match &self.addr {
            Address::Direct(addr) => *addr,
            Address::Domain(domain, port) => {
                let addr = self.resolver.resolve_one(domain).await?;
                SocketAddr::new(addr, *port)
            }
        };

        let stream = {
            let mut b = tcp::TcpStreamBuilder::new(addr).buff_size(self.buff_size);
            if let Some(timeout) = &self.timeout {
                b = b.timeout(*timeout);
            }
            b.build()?
        };

        let stream: tokio_rustls::client::TlsStream<TcpStream> = {
            let b = TlsConnectorBuilder::new().build()?;
            b.connect(Clone::clone(&self.sni), stream).await?
        };

        if log_enabled!(log::Level::Info) {
            let (stream, _) = stream.get_ref();
            info!(
                "establish pooled tls stream: {:?} -> {:?}",
                stream.local_addr().unwrap(),
                stream.peer_addr().unwrap()
            );
        }

        Ok(stream)
    }

    async fn recycle(&self, c: &mut Self::Type, metrics: &Metrics) -> RecycleResult<Self::Error> {
        let (stream, _) = c.get_ref();
        if let Err(e) = tcp::is_health(stream) {
            return Err(RecycleError::Backend(e));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use deadpool::Runtime;
    use futures::stream::StreamExt;
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::FramedRead;

    use super::*;

    #[tokio::test]
    async fn test_pool() -> Result<()> {
        use crate::protocol::http::{Flags as HttpCodecFlags, HttpCodec};

        const RAW_REQUEST: &[u8] = b"GET /anything HTTP/1.1\r\nAccept: *\r\nHost: httpbin.org\r\nUser-Agent: capybara/0.1.0\r\nConnection: keep-alive\r\n\r\n";

        pretty_env_logger::try_init_timed().ok();

        let closer = Arc::new(Notify::new());

        let pool = TlsStreamPoolBuilder::with_domain("httpbin.org", 443)
            .max_size(1)
            .build(Clone::clone(&closer))
            .await?;

        // Round#1
        {
            let mut c = pool.get().await.unwrap();

            assert!(is_health(&c).is_ok(), "socket should be healthy");

            let (r, mut w) = tokio::io::split(c.as_mut());
            let mut r = FramedRead::with_capacity(
                r,
                HttpCodec::new(HttpCodecFlags::RESPONSE, None, None),
                8192,
            );

            w.write_all(RAW_REQUEST).await?;

            let status_line = r.next().await;
            let headers = r.next().await;
            let body = r.next().await;

            info!("{:?}", status_line);
            info!("{:?}", headers);
            info!("{:?}", body);

            assert!(is_health(&c).is_ok(), "socket should be healthy");
        }

        // Round#2
        {
            let mut c = pool.get().await.unwrap();

            assert!(is_health(&c).is_ok(), "socket should be healthy");

            let (r, mut w) = tokio::io::split(c.as_mut());
            let mut r = FramedRead::with_capacity(
                r,
                HttpCodec::new(HttpCodecFlags::RESPONSE, None, None),
                8192,
            );

            w.write_all(RAW_REQUEST).await?;

            let status_line = r.next().await;
            let headers = r.next().await;
            let body = r.next().await;

            info!("{:?}", status_line);
            info!("{:?}", headers);
            info!("{:?}", body);

            assert!(is_health(&c).is_ok(), "socket should be healthy");
        }

        Ok(())
    }
}
