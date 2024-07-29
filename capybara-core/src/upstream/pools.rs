use std::sync::Arc;

use async_trait::async_trait;

use crate::transport::{tcp, tls};
use crate::Result;

pub enum Pool {
    Tcp(tcp::Pool),
    Tls(tls::Pool),
}

#[async_trait]
pub trait Pools: Send + Sync + 'static {
    async fn next(&self, seed: u64) -> Result<Arc<Pool>>;
}
