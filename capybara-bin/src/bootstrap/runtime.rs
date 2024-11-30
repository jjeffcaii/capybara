use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hashbrown::HashMap;
use tokio::sync::{mpsc, Notify, RwLock};

use capybara_core::logger;
use capybara_core::proto::{Addr, Listener, Signal};
use capybara_core::protocol::http::HttpListener;
use capybara_core::protocol::stream::StreamListener;
use capybara_core::transport::tcp::TcpStreamPoolBuilder;
use capybara_core::transport::tls::TlsStreamPoolBuilder;
use capybara_core::transport::TlsAcceptorBuilder;
use capybara_core::{Pool, Pools, RoundRobinPools, WeightedPools};
use capybara_etc::{
    BalanceStrategy, Config, ListenerConfig, Properties, TransportKind, UpstreamConfig,
};
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
                            let addr = endpoint.addr.parse::<Addr>()?;
                            let closer = Clone::clone(&self.closer);
                            if endpoint
                                .tls
                                .unwrap_or_else(|| endpoint.addr.ends_with(":443"))
                            {
                                Pool::Tls(TlsStreamPoolBuilder::new(addr).build(closer).await?)
                            } else {
                                Pool::Tcp(TcpStreamPoolBuilder::new(addr).build(closer).await?)
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
            BalanceStrategy::RoundRobin => {
                let mut pools: Vec<Arc<Pool>> = vec![];

                for endpoint in &v.endpoints {
                    let pool = match endpoint.transport.unwrap_or(v.transport) {
                        TransportKind::Tcp => {
                            let addr = endpoint.addr.parse::<Addr>()?;
                            let closer = Clone::clone(&self.closer);
                            if endpoint
                                .tls
                                .unwrap_or_else(|| endpoint.addr.ends_with(":443"))
                            {
                                Pool::Tls(TlsStreamPoolBuilder::new(addr).build(closer).await?)
                            } else {
                                Pool::Tcp(TcpStreamPoolBuilder::new(addr).build(closer).await?)
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

    fn get_duration(props: &Properties, key: &str) -> Option<Duration> {
        if let Some(val) = props.get(key) {
            if let Some(s) = val.as_str() {
                if let Ok(d) = duration_str::parse_std(s) {
                    return Some(d);
                }
            }
        }
        None
    }

    #[inline]
    async fn create_listener(&mut self, k: String, c: ListenerConfig) -> anyhow::Result<()> {
        let addr = c.listen.parse::<SocketAddr>()?;

        let listener: Arc<dyn Listener> = match c.protocol.name.as_str() {
            "http" => {
                let mut b = HttpListener::builder(addr).id(&k);

                if let Some(d) = Self::get_duration(&c.protocol.props, "client_header_timeout") {
                    b = b.client_header_timeout(d);
                }
                if let Some(d) = Self::get_duration(&c.protocol.props, "client_body_timeout") {
                    b = b.client_body_timeout(d);
                }
                if let Some(d) = Self::get_duration(&c.protocol.props, "keepalive_timeout") {
                    b = b.keepalive_timeout(d);
                }
                if let Some(d) = Self::get_duration(&c.protocol.props, "proxy_connect_timeout") {
                    b = b.proxy_connect_timeout(d);
                }
                if let Some(d) = Self::get_duration(&c.protocol.props, "proxy_read_timeout") {
                    b = b.proxy_read_timeout(d);
                }
                if let Some(d) = Self::get_duration(&c.protocol.props, "proxy_send_timeout") {
                    b = b.proxy_send_timeout(d);
                }

                for p in &c.pipelines {
                    b = b.pipeline(&p.name, &p.props);
                }

                for (k, v) in &self.upstreams {
                    b = b.upstream(k, Clone::clone(v));
                }

                Arc::new(b.build()?)
            }
            "https" => {
                let mut b = HttpListener::builder(addr).id(&k);

                let tls_acceptor = {
                    let mut tb = TlsAcceptorBuilder::default();

                    if let Some(path) = c.protocol.props.get("key_path") {
                        if let Some(key_path) = path.as_str() {
                            tb = tb.key_path(PathBuf::from(key_path));
                        }
                    }

                    if let Some(path) = c.protocol.props.get("cert_path") {
                        if let Some(cert_path) = path.as_str() {
                            tb = tb.cert_path(PathBuf::from(cert_path));
                        }
                    }

                    tb.build()?
                };

                b = b.tls(tls_acceptor);

                for p in &c.pipelines {
                    b = b.pipeline(&p.name, &p.props);
                }

                for (k, v) in &self.upstreams {
                    b = b.upstream(k, Clone::clone(v));
                }

                Arc::new(b.build()?)
            }
            "stream" => {
                let mut b = StreamListener::builder(addr).id(&k);

                for p in &c.pipelines {
                    b = b.pipeline(&p.name, &p.props);
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

pub(crate) struct Bootstrap {
    bc: BootstrapConf,
    c: Arc<RwLock<Config>>,
    dispatcher: Arc<RwLock<Dispatcher>>,
    loggers: Arc<RwLock<HashMap<String, logger::Key>>>,
}

impl Bootstrap {
    pub(crate) async fn shutdown(&self) {
        let mut w = self.dispatcher.write().await;
        w.shutdown().await;
    }

    async fn generate_loggers(&self) -> anyhow::Result<()> {
        let mut loggers: HashMap<String, logger::Key> = Default::default();
        let mut has_main = false;
        for (k, v) in &self.bc.loggers {
            if k == "main" {
                has_main = true;
                logger::init_global(v)?;
            } else {
                let lk = logger::register(v)?;
                loggers.insert(Clone::clone(k), lk);
            }
        }

        // use default logger settings if no main logger found
        if !has_main {
            logger::init_global(&logger::Config::default())?;
        }

        // extend loggers
        {
            let mut w = self.loggers.write().await;
            w.extend(loggers);
        }

        Ok(())
    }

    pub(crate) async fn start(&self) -> anyhow::Result<()> {
        self.generate_loggers().await?;

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
            loggers: Default::default(),
        }
    }
}
