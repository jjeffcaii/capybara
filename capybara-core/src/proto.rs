use async_trait::async_trait;

use crate::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    Shutdown,
    Reload,
}

pub type SignalReceiver = tokio::sync::mpsc::Receiver<Signal>;

pub trait Pipeline: 'static + Send + Sync {
    type Context;
    type Input;
    type Output;

    async fn handle(
        &mut self,
        ctx: &mut Self::Context,
        input: &mut Self::Input,
        output: &mut Self::Output,
    ) -> Result<()>;
}

pub trait Listener: Send + Sync + 'static {
    async fn listen(&self, signal: &mut SignalReceiver) -> Result<()>;
}

#[async_trait]
pub trait Configurable<T>: Send + Sync + 'static {
    async fn configure(&self, c: T) -> Result<()>;
}
