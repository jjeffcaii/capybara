use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;

use super::upstreams::Pool;

#[async_trait]
pub(crate) trait Pools: Send + Sync + 'static {
    async fn next(&self, seed: u64) -> Result<Arc<Pool>>;
}
