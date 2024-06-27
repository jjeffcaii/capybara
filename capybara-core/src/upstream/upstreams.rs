use std::sync::Arc;

use anyhow::Result;
use hashbrown::hash_map::Entry;
use hashbrown::HashMap;
use tokio::sync::Notify;
use tokio::sync::RwLock;

use crate::proto::UpstreamKey;
use crate::resolver::{Resolver, DEFAULT_RESOLVER};
use crate::transport::{tcp, tls};

pub(crate) enum Pool {
    Tcp(tcp::Pool),
    Tls(tls::Pool),
}

pub(crate) struct UpstreamsBuilder {
    inner: Upstreams,
}

impl UpstreamsBuilder {
    pub(crate) fn resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.inner.resolver.replace(resolver);
        self
    }

    pub(crate) fn build(self) -> Upstreams {
        self.inner
    }
}

#[derive(Clone)]
pub(crate) struct Upstreams {
    closer: Arc<Notify>,
    resolver: Option<Arc<dyn Resolver>>,
    inner: Arc<RwLock<HashMap<Arc<UpstreamKey>, Arc<Pool>>>>,
}

impl Upstreams {
    pub(crate) fn builder(closer: Arc<Notify>) -> UpstreamsBuilder {
        UpstreamsBuilder {
            inner: Self {
                inner: Default::default(),
                resolver: None,
                closer,
            },
        }
    }

    pub(crate) async fn get(&self, k: Arc<UpstreamKey>) -> Result<Arc<Pool>> {
        {
            let r = self.inner.read().await;
            if let Some(exist) = r.get(&k) {
                return Ok(Clone::clone(exist));
            }
        }

        let mut w = self.inner.write().await;

        match w.entry(k) {
            Entry::Occupied(ent) => Ok(Clone::clone(ent.get())),
            Entry::Vacant(ent) => {
                let pool = self.build_pool(ent.key()).await?;
                ent.insert(Clone::clone(&pool));
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
        };

        Ok(Arc::new(pool))
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_pools() {}
}
