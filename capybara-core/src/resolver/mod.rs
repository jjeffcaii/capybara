use std::net::IpAddr;
use std::sync::Arc;

use once_cell::sync::Lazy;

pub use dns::StandardDNSResolver;

use crate::error::CapybaraError::NoAddressResolved;
use crate::Result;

mod dns;

pub static DEFAULT_RESOLVER: Lazy<Arc<dyn Resolver>> =
    Lazy::new(|| Arc::new(StandardDNSResolver::default()));

#[async_trait::async_trait]
pub trait Resolver: Send + Sync + 'static {
    async fn resolve(&self, addr: &str) -> Result<Arc<Vec<IpAddr>>>;

    async fn resolve_one(&self, addr: &str) -> Result<IpAddr> {
        let all = self.resolve(addr).await?;
        all.first()
            .cloned()
            .ok_or_else(|| NoAddressResolved(addr.to_string().into()))
    }

    async fn is_valid(&self, addr: &str, ip: IpAddr) -> bool {
        true
    }
}
