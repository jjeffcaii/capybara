use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::upstream::Pool;
use crate::{CapybaraError, Result};

use super::pools::Pools;

pub struct RoundRobinPools {
    pools: Vec<Arc<Pool>>,
    seq: AtomicU32,
}

#[async_trait]
impl Pools for RoundRobinPools {
    async fn next(&self, seed: u64) -> Result<Arc<Pool>> {
        if self.pools.is_empty() {
            return Err(CapybaraError::InvalidUpstreamPool);
        }

        let bingo = (self.seq.fetch_add(1, Ordering::SeqCst) as usize) % self.pools.len();
        Ok(unsafe { Clone::clone(self.pools.get_unchecked(bingo)) })
    }
}

impl From<Vec<Arc<Pool>>> for RoundRobinPools {
    fn from(mut value: Vec<Arc<Pool>>) -> Self {
        use rand::seq::SliceRandom;
        use rand::thread_rng;
        let mut rng = thread_rng();
        value.shuffle(&mut rng);

        Self {
            pools: value,
            seq: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::tcp::TcpStreamPoolBuilder;
    use tokio::sync::Notify;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_round_robin() -> anyhow::Result<()> {
        init();

        let closer = Arc::new(Notify::new());

        let mut v = vec![];
        for next in ["httpbin.org", "httpbingo.org"] {
            let p = TcpStreamPoolBuilder::with_domain(next, 80)
                .build(Clone::clone(&closer))
                .await?;
            v.push(Arc::new(Pool::Tcp(p)));
        }

        let rrp = RoundRobinPools::from(v);

        assert!(rrp.next(0).await.is_ok());

        Ok(())
    }
}
