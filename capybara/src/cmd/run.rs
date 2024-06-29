use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Notify;

use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;

use crate::config::Config;

pub(crate) struct CommandRun {
    path: PathBuf,
}

impl CommandRun {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl super::Executable for CommandRun {
    async fn execute(&self, shutdown: Arc<Notify>) -> Result<()> {
        // TODO: implement run
        let c = {
            let b = tokio::fs::read(&self.path).await?;
            serde_yaml::from_slice::<Config>(&b[..])?
        };

        let mut signals = vec![];

        for (k, v) in c.listeners {
            match v.protocol.name.as_str() {
                "http" => {
                    let listener = {
                        let addr = v.listen.parse::<SocketAddr>()?;
                        let mut b = HttpListener::builder(addr).id(&k);

                        for p in &v.pipelines {
                            b = b.pipeline(&p.name, &p.props);
                        }
                        b.build()?
                    };

                    let (tx, mut rx) = tokio::sync::mpsc::channel::<Signal>(1);
                    tokio::spawn(async move {
                        if let Err(e) = listener.listen(&mut rx).await {
                            error!("listener '{}' is stopped: {:?}", k, e);
                        }
                    });
                    signals.push(tx);
                }
                unknown => {
                    bail!("invalid protocol '{}'", unknown);
                }
            }
        }

        shutdown.notified().await;
        for next in signals {
            next.send(Signal::Shutdown).await.ok();
        }

        Ok(())
    }
}
