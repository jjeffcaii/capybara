use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use hashbrown::HashMap;
use tokio::sync::{mpsc, Notify, RwLock};

use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;
use capybara_core::transport::tcp::TcpStreamPoolBuilder;
use capybara_core::transport::tls::TlsStreamPoolBuilder;
use capybara_core::{CapybaraError, Pool, Pools, RoundRobinPools, WeightedPools};
use capybara_etc::{BalanceStrategy, Config, ListenerConfig, TransportKind, UpstreamConfig};
use capybara_util::WeightedResource;

use crate::provider::{ConfigProvider, StaticFileWatcher};

use super::config::BootstrapConf;

enum ConfigOperation<T> {
    Remove(String),
    Create(String, T),
    Update(String, T),
}

#[derive(Default)]
struct Dispatcher {
    closer: Arc<Notify>,
    listeners: HashMap<String, (mpsc::Sender<Signal>, Arc<dyn Listener>)>,
    upstreams: HashMap<String, Arc<dyn Pools>>,
}

impl Dispatcher {
    async fn shutdown(&mut self) {
        for (_, (tx, _)) in &self.listeners {
            tx.send(Signal::Shutdown).await.ok();
        }
    }

    async fn dispatch_upstream(&mut self, op: ConfigOperation<UpstreamConfig>) {
        match op {
            ConfigOperation::Remove(k) => {
                self.remove_listener(&k).await.ok();
            }
            ConfigOperation::Create(k, v) => {
                self.create_upstream(k, v).await.ok();
            }
            ConfigOperation::Update(_k, _v) => {
                todo!("update listener")
            }
        }
    }

    async fn create_upstream(&mut self, k: String, v: UpstreamConfig) -> anyhow::Result<()> {
        let p: Arc<dyn Pools> = match v.balancer {
            BalanceStrategy::Weighted => {
                let mut b = WeightedResource::builder();

                for endpoint in &v.endpoints {
                    let weight = endpoint.weight.unwrap_or(10);

                    let p = match endpoint.transport.as_ref().unwrap_or(&v.transport) {
                        TransportKind::Tcp => {
                            if endpoint.tls.is_some_and(|it| it) || endpoint.addr.ends_with(":443")
                            {
                                let b = to_tls_stream_pool_builder(&endpoint.addr)?;
                                Pool::Tls(b.build(Clone::clone(&self.closer)).await?)
                            } else {
                                let b = to_tcp_stream_pool_builder(&endpoint.addr)?;
                                Pool::Tcp(b.build(Clone::clone(&self.closer)).await?)
                            }
                        }
                        TransportKind::Udp => {
                            todo!()
                        }
                    };

                    b = b.push(weight, p.into());
                }
                Arc::new(WeightedPools::from(b.build()))
            }
            BalanceStrategy::IpHash => {
                todo!()
            }
            BalanceStrategy::RoundRobin => {
                let mut pools: Vec<Arc<Pool>> = vec![];

                for endpoint in &v.endpoints {
                    let pool = match endpoint.transport.unwrap_or(v.transport) {
                        TransportKind::Tcp => {
                            if endpoint.tls.is_some_and(|it| it) || endpoint.addr.ends_with(":443")
                            {
                                let b = to_tls_stream_pool_builder(&endpoint.addr)?;
                                let p = b.build(Clone::clone(&self.closer)).await?;
                                Pool::Tls(p)
                            } else {
                                let b = to_tcp_stream_pool_builder(&endpoint.addr)?;
                                let p = b.build(Clone::clone(&self.closer)).await?;
                                Pool::Tcp(p)
                            }
                        }
                        TransportKind::Udp => {
                            todo!()
                        }
                    };

                    pools.push(pool.into());
                }
                Arc::new(RoundRobinPools::from(pools))
            }
        };

        self.upstreams.insert(k, p);

        Ok(())
    }

    async fn dispatch_listener(&mut self, op: ConfigOperation<ListenerConfig>) {
        match op {
            ConfigOperation::Remove(k) => {
                self.remove_listener(&k).await.ok();
            }
            ConfigOperation::Create(k, v) => {
                self.create_listener(k, v).await.ok();
            }
            ConfigOperation::Update(_k, _v) => {
                todo!("update listener")
            }
        }
    }

    async fn remove_listener(&mut self, k: &String) -> anyhow::Result<()> {
        if let Some((tx, _)) = self.listeners.remove(k) {
            tx.send(Signal::Shutdown).await?;
        }
        Ok(())
    }

    #[inline]
    async fn create_listener(&mut self, k: String, c: ListenerConfig) -> anyhow::Result<()> {
        let listener: Arc<dyn Listener> = match c.protocol.name.as_str() {
            "http" => {
                let addr = c.listen.parse::<SocketAddr>()?;
                let mut b = HttpListener::builder(addr).id(&k);

                for p in &c.pipelines {
                    b = b.pipeline(&p.name, &p.props);
                }

                for (k, v) in &self.upstreams {
                    b = b.upstream(k, Clone::clone(v));
                }

                Arc::new(b.build()?)
            }
            other => {
                bail!("invalid listener protocol '{}'", other);
            }
        };

        let (tx, mut rx) = mpsc::channel::<Signal>(1);

        {
            let listener = Clone::clone(&listener);
            tokio::spawn(async move {
                if let Err(e) = listener.listen(&mut rx).await {
                    error!("listener '{}' is stopped: {:?}", listener.id(), e);
                }
            });
        }

        self.listeners.insert(k, (tx, listener));

        Ok(())
    }
}

#[inline]
fn to_tls_stream_pool_builder(addr: &str) -> anyhow::Result<TlsStreamPoolBuilder> {
    let mut sp = addr.split(':');
    if let Some(left) = sp.next() {
        if let Some(right) = sp.next() {
            if let Ok(port) = right.parse::<u16>() {
                if sp.next().is_none() {
                    return Ok(match left.parse::<IpAddr>() {
                        Ok(ip) => TlsStreamPoolBuilder::with_addr(SocketAddr::new(ip, port)),
                        Err(_) => TlsStreamPoolBuilder::with_domain(left, port),
                    });
                }
            }
        }
    }

    bail!(CapybaraError::InvalidUpstream(addr.to_string().into()));
}

#[inline]
fn to_tcp_stream_pool_builder(addr: &str) -> anyhow::Result<TcpStreamPoolBuilder> {
    let mut sp = addr.split(':');
    if let Some(left) = sp.next() {
        if let Some(right) = sp.next() {
            if let Ok(port) = right.parse::<u16>() {
                if sp.next().is_none() {
                    return Ok(match left.parse::<IpAddr>() {
                        Ok(ip) => TcpStreamPoolBuilder::with_addr(SocketAddr::new(ip, port)),
                        Err(_) => TcpStreamPoolBuilder::with_domain(left, port),
                    });
                }
            }
        }
    }

    bail!(CapybaraError::InvalidUpstream(addr.to_string().into()));
}

pub(crate) struct Bootstrap {
    bc: BootstrapConf,
    c: Arc<RwLock<Config>>,
    dispatcher: Arc<RwLock<Dispatcher>>,
}

impl Bootstrap {
    pub(crate) async fn shutdown(&self) {
        let mut w = self.dispatcher.write().await;
        w.shutdown().await;
    }

    pub(crate) async fn start(&self) -> anyhow::Result<()> {
        let (c_tx, mut c_rx) = mpsc::unbounded_channel::<Config>();

        {
            let dispatcher = Clone::clone(&self.dispatcher);
            let c = Clone::clone(&self.c);
            tokio::spawn(async move {
                while let Some(next) = c_rx.recv().await {
                    let mut prev = c.write().await;

                    for k in prev.upstreams.keys() {
                        if !next.upstreams.contains_key(k) {
                            let mut d = dispatcher.write().await;
                            d.dispatch_upstream(ConfigOperation::Remove(Clone::clone(k)))
                                .await;
                        }
                    }

                    // CREATE: $not_exist(prev) && $exit(next)
                    // UPDATE: $exist(prev) && $exist(next)
                    for (k, v) in &next.upstreams {
                        match prev.upstreams.get(k) {
                            None => {
                                let op = ConfigOperation::Create(Clone::clone(k), Clone::clone(v));
                                {
                                    let mut d = dispatcher.write().await;
                                    d.dispatch_upstream(op).await;
                                }
                                prev.upstreams.insert(Clone::clone(k), Clone::clone(v));
                            }
                            Some(exist) if exist != v => {
                                {
                                    let mut d = dispatcher.write().await;
                                    d.dispatch_upstream(ConfigOperation::Update(
                                        Clone::clone(k),
                                        Clone::clone(v),
                                    ))
                                    .await;
                                }
                                prev.upstreams.insert(Clone::clone(k), Clone::clone(v));
                            }
                            _ => (),
                        }
                    }

                    // REMOVE: $exist(prev) && $not_exist(next)
                    for k in prev.listeners.keys() {
                        if !next.listeners.contains_key(k) {
                            let mut d = dispatcher.write().await;
                            d.dispatch_listener(ConfigOperation::Remove(Clone::clone(k)))
                                .await;
                        }
                    }

                    // CREATE: $not_exist(prev) && $exit(next)
                    // UPDATE: $exist(prev) && $exist(next)
                    for (k, v) in &next.listeners {
                        match prev.listeners.get(k) {
                            None => {
                                {
                                    let mut d = dispatcher.write().await;
                                    d.dispatch_listener(ConfigOperation::Create(
                                        Clone::clone(k),
                                        Clone::clone(v),
                                    ))
                                    .await;
                                }
                                prev.listeners.insert(Clone::clone(k), Clone::clone(v));
                            }
                            Some(exist) if exist != v => {
                                {
                                    let mut d = dispatcher.write().await;
                                    d.dispatch_listener(ConfigOperation::Update(
                                        Clone::clone(k),
                                        Clone::clone(v),
                                    ))
                                    .await;
                                }
                                prev.listeners.insert(Clone::clone(k), Clone::clone(v));
                            }
                            _ => (),
                        }
                    }
                }
            });
        }

        for c in &self.bc.providers {
            match c.kind.as_str() {
                "static_file" => {
                    if let Some(val) = c.props.get("path") {
                        if let Some(path) = val.as_str() {
                            let pb = match path.strip_prefix("~/") {
                                Some(rel) => {
                                    let pb = PathBuf::from(rel);
                                    dirs::home_dir().unwrap_or_default().join(pb)
                                }
                                None => PathBuf::from(path),
                            };
                            let w = StaticFileWatcher::new(pb);

                            let tx = Clone::clone(&c_tx);
                            tokio::spawn(async move {
                                if let Err(e) = w.watch(tx).await {
                                    error!("static_file watcher is stopped: {}", e);
                                }
                            });
                        }
                    }
                }
                other => bail!("invalid provider kind '{}'!", other),
            }
        }

        Ok(())
    }
}

impl From<BootstrapConf> for Bootstrap {
    fn from(value: BootstrapConf) -> Self {
        Self {
            bc: value,
            c: Default::default(),
            dispatcher: Default::default(),
        }
    }
}
