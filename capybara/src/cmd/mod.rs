use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Notify;

pub(crate) use run::CommandRun;

mod run;

#[async_trait::async_trait]
pub(crate) trait Executable: 'static + Send + Sync {
    async fn execute(&self, shutdown: Arc<Notify>) -> Result<()>;
}
