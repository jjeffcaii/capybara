use async_trait::async_trait;

use crate::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    Shutdown,
    Reload,
}

pub type Signals = tokio::sync::mpsc::Receiver<Signal>;

#[async_trait]
pub trait Listener: Send + Sync + 'static {
    async fn listen(&self, signals: &mut Signals) -> Result<()>;
}

#[async_trait]
pub trait Configurable<T>: Send + Sync + 'static {
    async fn configure(&self, c: T) -> Result<()>;
}
