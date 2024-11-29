use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Notify;

use crate::bootstrap::{Bootstrap, BootstrapConf};

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
        let bc = {
            let b = tokio::fs::read(&self.path).await?;
            serde_yaml::from_slice::<BootstrapConf>(&b[..])?
        };

        let bt = Bootstrap::from(bc);

        bt.start().await?;

        shutdown.notified().await;
        bt.shutdown().await;

        Ok(())
    }
}
