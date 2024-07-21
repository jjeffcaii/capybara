use std::ffi::CStr;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;

use foreign_types::{foreign_type, ForeignType, ForeignTypeRef};

foreign_type! {
    pub unsafe type IfAddrs: Sync+Send {
        type CType = libc::ifaddrs;
        fn drop = libc::freeifaddrs;
    }
}

impl IfAddrs {
    pub fn get() -> io::Result<IfAddrs> {
        unsafe {
            let mut ifaddrs = ptr::null_mut();
            let r = libc::getifaddrs(&mut ifaddrs);
            if r == 0 {
                Ok(IfAddrs::from_ptr(ifaddrs))
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

impl IfAddrsRef {
    pub fn next(&self) -> Option<&IfAddrsRef> {
        unsafe {
            let next = (*self.as_ptr()).ifa_next;
            if next.is_null() {
                None
            } else {
                Some(IfAddrsRef::from_ptr(next))
            }
        }
    }

    pub fn name(&self) -> &str {
        unsafe {
            let s = CStr::from_ptr((*self.as_ptr()).ifa_name);
            s.to_str().unwrap()
        }
    }

    pub fn addr(&self) -> Option<IpAddr> {
        unsafe {
            let addr = (*self.as_ptr()).ifa_addr;
            if addr.is_null() {
                return None;
            }

            match (*addr).sa_family as _ {
                libc::AF_INET => {
                    let addr = addr as *mut libc::sockaddr_in;
                    // It seems like this to_be shouldn't be needed?
                    let addr = Ipv4Addr::from((*addr).sin_addr.s_addr.to_be());
                    Some(IpAddr::V4(addr))
                }
                libc::AF_INET6 => {
                    let addr = addr as *mut libc::sockaddr_in6;
                    let addr = Ipv6Addr::from((*addr).sin6_addr.s6_addr);
                    Some(IpAddr::V6(addr))
                }
                _ => None,
            }
        }
    }

    pub fn iter(&self) -> Iter {
        Iter(Some(self))
    }
}

impl<'a> IntoIterator for &'a IfAddrs {
    type Item = &'a IfAddrsRef;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a IfAddrsRef {
    type Item = &'a IfAddrsRef;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

pub struct Iter<'a>(Option<&'a IfAddrsRef>);

impl<'a> Iterator for Iter<'a> {
    type Item = &'a IfAddrsRef;

    fn next(&mut self) -> Option<&'a IfAddrsRef> {
        let cur = match self.0 {
            Some(cur) => cur,
            None => return None,
        };

        self.0 = cur.next();
        Some(cur)
    }
}

#[cfg(test)]
mod tests {
    use log::info;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_ifaddrs() {
        init();

        let addrs = IfAddrs::get();
        assert!(addrs.is_ok());

        addrs
            .unwrap()
            .iter()
            .map(|it| (it.name(), it.addr()))
            .for_each(|(name, addr)| {
                info!("{}: {:?}", name, addr);
            });
    }
}
