use std::io;
use std::net::{IpAddr, SocketAddr};
use std::result::Result as StdResult;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ahash::HashSet;
use hickory_resolver::config::{
    NameServerConfig, NameServerConfigGroup, Protocol, ResolverConfig, ResolverOpts,
};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::AsyncResolver;
use hickory_resolver::TokioAsyncResolver;
use moka::future::Cache;
use once_cell::sync::Lazy;
use rand::Rng;
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::{CapybaraError, Result};

use super::Resolver;

static RESOLVER: Lazy<AsyncResolver<TokioConnectionProvider>> = Lazy::new(|| {
    if let Ok(s) = std::env::var("CAPYBARA_DNS") {
        let mut nsc = vec![];
        for next in s
            .split([';', ','])
            .map(|it| it.trim())
            .filter(|it| !it.is_empty())
        {
            if let Ok(ipaddr) = next.parse::<IpAddr>() {
                nsc.push(NameServerConfig::new(
                    SocketAddr::new(ipaddr, 53),
                    Protocol::Udp,
                ));
                continue;
            }
            if let Ok(socketaddr) = next.parse::<SocketAddr>() {
                nsc.push(NameServerConfig::new(socketaddr, Protocol::Udp));
                continue;
            }
        }

        if !nsc.is_empty() {
            return TokioAsyncResolver::new(
                ResolverConfig::from_parts(None, vec![], NameServerConfigGroup::from(nsc)),
                ResolverOpts::default(),
                TokioConnectionProvider::default(),
            );
        }
    }

    TokioAsyncResolver::from_system_conf(TokioConnectionProvider::default()).unwrap()
});

/// The DNS cache size.
static DNS_CACHE_SIZE: Lazy<usize> = Lazy::new(|| {
    let mut size = 1000usize;
    if let Ok(s) = std::env::var("CAPYBARA_DNS_CACHE_SIZE") {
        if let Ok(n) = s.parse::<usize>() {
            if n > 0 {
                size = n;
            }
        }
    }
    size
});

/// The refresh period in seconds.
static DNS_CACHE_REFRESH_SECS: Lazy<usize> = Lazy::new(|| {
    let mut period = 5usize;
    if let Ok(s) = std::env::var("CAPYBARA_DNS_CACHE_REFRESH_SECS") {
        if let Ok(n) = s.parse::<usize>() {
            if n > 0 {
                period = n;
            }
        }
    }
    period
});

/// Refresher will refresh all DNS cache in period.
#[derive(Clone)]
struct Refresher {
    refreshing: Arc<AtomicBool>,
    ttl_high_water: Duration,
    cache: Cache<String, DNSContext, ahash::RandomState>,
}

impl Refresher {
    async fn refresh_all(&self) {
        match self
            .refreshing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Acquire)
        {
            Ok(false) => {
                // make a join set
                let mut joins = JoinSet::new();

                // collect those expired dns cache.
                for (k, c) in self.cache.iter() {
                    let elapsed = c.t.elapsed();
                    if elapsed > self.ttl_high_water {
                        let addr = k.to_string();
                        let cache = self.cache.clone();
                        joins.spawn(async move {
                            if let Ok(val) = inner_resolve(&addr).await {
                                cache.insert(addr, val).await;
                            }
                        });
                    }
                }

                // join all
                let mut cnt = 0;
                while let Some(Ok(())) = joins.join_next().await {
                    cnt += 1;
                }

                self.refreshing.store(false, Ordering::SeqCst);

                debug!("refresh address finish: cnt={}", cnt);
            }
            _ => debug!("skip refreshing because previous operation is not finished"),
        }
    }
}

#[derive(Clone)]
struct DNSContext {
    v: Arc<Vec<IpAddr>>,
    s: Arc<HashSet<IpAddr>>,
    t: Instant,
}

/// A resolver which cache DNS records in memory.
pub struct StandardDNSResolver {
    closed: Arc<Notify>,
    cache: Cache<String, DNSContext, ahash::RandomState>,
}

impl StandardDNSResolver {
    pub fn new(cap: usize, ttl_secs: usize) -> StandardDNSResolver {
        let ttl = Duration::from_secs(usize::max(1, ttl_secs) as u64);
        let cache = Cache::builder()
            .max_capacity(cap as u64)
            .time_to_live(ttl)
            .build_with_hasher(ahash::RandomState::default());

        let closed = Arc::new(Notify::new());

        let r = Self {
            closed: Clone::clone(&closed),
            cache: Clone::clone(&cache),
        };

        let refresher = Refresher {
            refreshing: Default::default(),
            ttl_high_water: ttl * 4 / 5,
            cache,
        };

        let period = {
            // basic period is 1s at least .
            let basic = Duration::max(ttl * 2 / 5, Duration::from_secs(1));
            // add a random extra period 10ms ~ 100ms.
            let extra = Duration::from_millis(rand::thread_rng().gen_range(10..100));
            basic + extra
        };

        let mut tk = {
            use tokio::time;
            time::interval_at(time::Instant::now() + period, period)
        };

        tokio::spawn(async move {
            loop {
                tokio::select! {
                  _ = closed.notified() => {
                        info!("refresh ticker of cached dns resolver is stopped!");
                        break;
                  }
                  _ = tk.tick() => {
                        let refresher = Clone::clone(&refresher);
                        tokio::spawn(async move {
                            refresher.refresh_all().await;
                        });
                    }
                }
            }
        });

        r
    }
}

impl Drop for StandardDNSResolver {
    fn drop(&mut self) {
        self.closed.notify_one();
    }
}

impl Default for StandardDNSResolver {
    fn default() -> Self {
        StandardDNSResolver::new(*DNS_CACHE_SIZE, *DNS_CACHE_REFRESH_SECS)
    }
}

#[async_trait::async_trait]
impl Resolver for StandardDNSResolver {
    async fn resolve(&self, addr: &str) -> Result<Arc<Vec<IpAddr>>> {
        match self.cache.get(addr).await {
            Some(ctx) => Ok(ctx.v),
            None => {
                match self
                    .cache
                    .try_get_with(addr.to_string(), inner_resolve(addr))
                    .await
                {
                    Ok(c) => Ok(c.v),
                    Err(e) => {
                        error!("failed to resolve address '{}': {:?}", addr, e);
                        Err(CapybaraError::NoAddressResolved(addr.to_string().into()))
                    }
                }
            }
        }
    }

    async fn is_valid(&self, addr: &str, ip: IpAddr) -> bool {
        if let Some(c) = self.cache.get(addr).await {
            return c.s.contains(&ip);
        }
        false
    }
}

#[inline]
async fn inner_resolve(addr: &str) -> StdResult<DNSContext, io::Error> {
    let ips = RESOLVER.lookup_ip(addr).await?;

    let v: Vec<IpAddr> = ips.iter().collect();
    let s: HashSet<IpAddr> = v.iter().cloned().collect();

    Ok(DNSContext {
        v: Arc::new(v),
        s: Arc::new(s),
        t: Instant::now(),
    })
}

#[cfg(test)]
mod dns_resolver_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn resolve() {
        init();

        let r = StandardDNSResolver::new(128, 3);

        for i in 0..3 {
            let resolved = r.resolve("youtube.com.").await;
            assert!(resolved.is_ok_and(|next| {
                info!("next: {:?}", next);
                !next.is_empty()
            }));
        }

        tokio::time::sleep(Duration::from_secs(4)).await;
        drop(r);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
