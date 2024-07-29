use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use capybara_etc::Config;

use super::ConfigProvider;

pub(crate) struct StaticFileWatcher {
    path: PathBuf,
}

impl StaticFileWatcher {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl ConfigProvider for StaticFileWatcher {
    type Item = Config;

    async fn watch(&self, tx: UnboundedSender<Self::Item>) -> anyhow::Result<()> {
        let item = {
            use tokio::fs::File;
            use tokio::io::AsyncReadExt;

            let mut f = File::open(&self.path).await?;

            let mut contents = vec![];
            f.read_to_end(&mut contents).await?;

            serde_yaml::from_slice::<'_, Self::Item>(&contents[..])?
        };

        tx.send(item)?;

        Ok(())
    }
}
