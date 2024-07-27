use tokio::sync::mpsc::UnboundedSender;

pub(crate) use static_file::StaticFileWatcher;

mod static_file;

#[async_trait::async_trait]
pub trait ConfigProvider: Send + Sync + 'static {
    type Item;

    async fn watch(&self, tx: UnboundedSender<Self::Item>) -> anyhow::Result<()>;
}
