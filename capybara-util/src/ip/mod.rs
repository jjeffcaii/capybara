use std::net::IpAddr;

use once_cell::sync::Lazy;

mod ifaddrs;

pub(crate) static IP: Lazy<Option<IpAddr>> = Lazy::new(|| {
    if let Ok(addrs) = ifaddrs::IfAddrs::get() {
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

pub fn local() -> Option<IpAddr> {
    Clone::clone(&IP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip() {
        pretty_env_logger::try_init_timed().ok();
        log::info!("ip: {:?}", &*IP);
        assert!(IP.is_some());
    }
}
