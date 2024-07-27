use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use hashbrown::HashMap;
use tokio::sync::{mpsc, RwLock};

use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;

use crate::config::{Config, ListenerConfig};
use crate::provider::{ConfigProvider, StaticFileWatcher};

use super::config::BootstrapConf;

enum ConfigOperation<T> {
    Remove(String),
    Create(String, T),
    Update(String, T),
}

#[derive(Default)]
struct Dispatcher {
    listeners: HashMap<String, (mpsc::Sender<Signal>, Arc<dyn Listener>)>,
}

impl Dispatcher {
    async fn shutdown(&mut self) {
        for (_, (tx, _)) in &self.listeners {
            tx.send(Signal::Shutdown).await.ok();
        }
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
