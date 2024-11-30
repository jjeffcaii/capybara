use std::sync::Arc;

use async_trait::async_trait;

use capybara_util::WeightedResource;

use crate::{CapybaraError, Result};

use super::pools::{Pool, Pools};

pub struct WeightedPools(WeightedResource<Arc<Pool>>);

#[async_trait]
impl Pools for WeightedPools {
    async fn next(&self, _: u64) -> Result<Arc<Pool>> {
        match self.0.next() {
            None => Err(CapybaraError::InvalidUpstreamPool),
            Some(next) => Ok(Clone::clone(next)),
        }
    }
}

impl From<WeightedResource<Arc<Pool>>> for WeightedPools {
    fn from(value: WeightedResource<Arc<Pool>>) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::Addr;
    use crate::transport::tcp::TcpStreamPoolBuilder;
    use tokio::sync::Notify;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_weighted_pools() -> anyhow::Result<()> {
        init();

        let closer = Arc::new(Notify::new());

        let new_pool = |domain: &str| {
            let closer = Clone::clone(&closer);
            let addr = format!("{}:80", domain).parse::<Addr>().unwrap();
            let bu = TcpStreamPoolBuilder::new(addr);
            async { bu.build(closer).await.map(|it| Arc::new(Pool::Tcp(it))) }
        };

        let pools = {
            let p = WeightedResource::<Arc<Pool>>::builder()
                .push(50, new_pool("httpbin.org").await?)
                .push(50, new_pool("httpbingo.org").await?)
                .build();
            WeightedPools::from(p)
        };

        assert!(pools.next(0).await.is_ok());

        Ok(())
    }
}
