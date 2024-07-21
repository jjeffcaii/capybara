use std::net::IpAddr;

use once_cell::sync::Lazy;

pub use ifaddrs::IfAddrs;
pub use rotate::{FileRotate, RotationMode};

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

mod ifaddrs;
mod rotate;

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
