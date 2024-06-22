use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Notify;

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
    async fn execute(&self, _shutdown: Arc<Notify>) -> Result<()> {
        // TODO: implement run
        let _ = self.path;
        bail!("unimplemented command: ")
    }
}
