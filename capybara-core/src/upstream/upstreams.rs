use std::sync::Arc;

use anyhow::Result;
use hashbrown::hash_map::Entry;
use hashbrown::HashMap;
use tokio::sync::Notify;
use tokio::sync::RwLock;

use crate::cachestr::Cachestr;
use crate::proto::UpstreamKey;
use crate::resolver::{Resolver, DEFAULT_RESOLVER};
use crate::transport::{tcp, tls};
use crate::upstream::Pools;

use super::pools::Pool;

pub(crate) struct UpstreamsBuilder {
    closer: Arc<Notify>,
    resolver: Option<Arc<dyn Resolver>>,
    pools: HashMap<Cachestr, Arc<dyn Pools>>,
}

impl UpstreamsBuilder {
    pub(crate) fn resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver.replace(resolver);
        self
    }

    pub(crate) fn keyed(mut self, key: Cachestr, value: Arc<dyn Pools>) -> Self {
        self.pools.insert(key, value);
        self
    }

    pub(crate) fn build(self) -> Upstreams {
        let Self {
            closer,
            resolver,
            pools,
        } = self;
        let mut kv: HashMap<Arc<UpstreamKey>, UpstreamValue> = Default::default();

        for (k, v) in pools {
            kv.insert(UpstreamKey::Tag(k).into(), UpstreamValue::Pools(v));
        }

        Upstreams {
            resolver,
            closer,
            inner: Arc::new(RwLock::new(kv)),
        }
    }
}

enum UpstreamValue {
    Direct(Arc<Pool>),
    Pools(Arc<dyn Pools>),
}

#[derive(Clone)]
pub(crate) struct Upstreams {
    closer: Arc<Notify>,
    resolver: Option<Arc<dyn Resolver>>,
    inner: Arc<RwLock<HashMap<Arc<UpstreamKey>, UpstreamValue>>>,
}

impl Upstreams {
    pub(crate) fn builder(closer: Arc<Notify>) -> UpstreamsBuilder {
        UpstreamsBuilder {
            pools: Default::default(),
            closer,
            resolver: None,
        }
    }

    pub(crate) async fn get(&self, k: Arc<UpstreamKey>, seed: u64) -> Result<Arc<Pool>> {
        {
            let r = self.inner.read().await;
            if let Some(exist) = r.get(&k) {
                return match exist {
                    UpstreamValue::Direct(it) => Ok(Clone::clone(it)),
                    UpstreamValue::Pools(pools) => pools.next(seed).await.map_err(|e| e.into()),
                };
            }
        }

        let mut w = self.inner.write().await;

        match w.entry(k) {
            Entry::Occupied(ent) => match ent.get() {
                UpstreamValue::Direct(it) => Ok(Clone::clone(it)),
                UpstreamValue::Pools(pools) => pools.next(seed).await.map_err(|e| e.into()),
            },
            Entry::Vacant(ent) => {
                let pool = self.build_pool(ent.key()).await?;
                ent.insert(UpstreamValue::Direct(Clone::clone(&pool)));
                Ok(pool)
            }
        }
    }

    #[inline]
    async fn build_pool(&self, k: &UpstreamKey) -> Result<Arc<Pool>> {
        let closer = Clone::clone(&self.closer);

        let pool = match k {
            UpstreamKey::Tcp(addr) => {
                let p = tcp::TcpStreamPoolBuilder::with_addr(*addr)
                    .build(closer)
                    .await?;
                Pool::Tcp(p)
            }
            UpstreamKey::Tls(addr, sni) => {
                let p = tls::TlsStreamPoolBuilder::with_addr(*addr)
                    .sni(Clone::clone(sni))
                    .build(closer)
                    .await?;
                Pool::Tls(p)
            }
            UpstreamKey::TcpHP(host, port) => {
                let resolver = match &self.resolver {
                    None => Clone::clone(&*DEFAULT_RESOLVER),
                    Some(resolver) => Clone::clone(resolver),
                };

                let p = tcp::TcpStreamPoolBuilder::with_domain(host.as_ref(), *port)
                    .resolver(resolver)
                    .build(closer)
                    .await?;
                Pool::Tcp(p)
            }
            UpstreamKey::TlsHP(host, port, sni) => {
                let resolver = match &self.resolver {
                    None => Clone::clone(&*DEFAULT_RESOLVER),
                    Some(resolver) => Clone::clone(resolver),
                };

                let p = tls::TlsStreamPoolBuilder::with_domain(host.as_ref(), *port)
                    .resolver(resolver)
                    .build(closer)
                    .await?;
                Pool::Tls(p)
            }
            UpstreamKey::Tag(tag) => {
                bail!("no upstream tag '{}' found", tag.as_ref());
            }
        };

        Ok(Arc::new(pool))
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_pools() {}
}
