#![allow(clippy::type_complexity)]
#![allow(clippy::from_over_into)]
#![allow(clippy::module_inception)]
#![allow(clippy::upper_case_acronyms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

use std::net::IpAddr;

use once_cell::sync::Lazy;

pub use ifaddrs::IfAddrs;
pub use rotate::{FileRotate, RotationMode};
pub use weighted::{WeightedResource, WeightedResourceBuilder};

static IP: Lazy<Option<IpAddr>> = Lazy::new(|| {
    if let Ok(addrs) = IfAddrs::get() {
        for next in addrs.iter() {
            if let Some(addr) = next.addr() {
                if addr.is_ipv4() && !addr.is_loopback() {
                    return Some(addr);
                }
            }
        }
    }

    None
});

pub fn local_addr() -> Option<IpAddr> {
    Clone::clone(&IP)
}

/// cached string
pub mod cachestr {
    include!(concat!(env!("OUT_DIR"), "/cachestr.rs"));
}

mod ifaddrs;
mod rotate;
mod weighted;

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_local_addr() {
        init();
        log::info!("local_addr: {:?}", &*IP);
        assert!(IP.is_some());
    }
}
