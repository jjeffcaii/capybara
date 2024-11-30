use std::net::SocketAddr;
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Type};
use tokio::net::{TcpListener, TcpStream};

use crate::error::CapybaraError;
use crate::Result;

pub struct TcpListenerBuilder {
    addr: SocketAddr,
    buff_size: usize,
    reuse: bool,
}

impl TcpListenerBuilder {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            buff_size: 0,
            reuse: true,
        }
    }

    pub fn buff_size(mut self, buff_size: usize) -> Self {
        self.buff_size = buff_size;
        self
    }

    pub fn reuse(mut self, reuse: bool) -> Self {
        self.reuse = reuse;
        self
    }

    pub fn build(&self) -> Result<TcpListener> {
        listen(self.addr, self.buff_size, self.reuse)
    }
}

#[inline]
fn listen(addr: SocketAddr, buff_size: usize, reuse: bool) -> Result<TcpListener> {
    let socket = match &addr {
        SocketAddr::V4(_) => socket2::Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?,
        SocketAddr::V6(_) => socket2::Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?,
    };

    if reuse {
        // SO_REUSEADDR+SO_REUSEPORT
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;

        // enable balance for freebsd
        cfg_if! {
            if #[cfg(target_os="freebsd")] {
                socket.set_reuse_port_lb(true)?;
            }
        }
    }

    if buff_size > 0 {
        socket.set_recv_buffer_size(buff_size)?;
        socket.set_send_buffer_size(buff_size)?;
    }

    socket.set_nonblocking(true)?;
    socket.set_nodelay(true)?;

    // set sock opts for TPROXY on linux, it requires CAP_NET_ADMIN
    cfg_if! {
        if #[cfg(target_os="linux")] {
            if let Err(e) = match &addr {
                SocketAddr::V4(_) => socket.set_freebind(true),
                SocketAddr::V6(_) => socket.set_freebind_ipv6(true),
            } {
                warn!("failed to set socket {:?} opt SOL_IP+IP_FREEBIND: {}", &addr, e);
            }
            if let Err(e) = socket.set_ip_transparent(true) {
                warn!("failed to set socket {:?} opt IP_TRANSPARENT: {}", &addr, e);
            }


        }
    }

    socket.bind(&addr.into())?;
    socket.listen(65535)?;

    Ok(TcpListener::from_std(socket.into())?)
}

pub struct TcpStreamBuilder {
    laddr: Option<SocketAddr>,
    addr: SocketAddr,
    timeout: Option<Duration>,
    buff_size: usize,
}

impl TcpStreamBuilder {
    pub const BUFF_SIZE: usize = 8192;

    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            laddr: None,
            timeout: None,
            buff_size: Self::BUFF_SIZE,
        }
    }

    pub fn local_addr(mut self, addr: SocketAddr) -> Self {
        self.laddr.replace(addr);
        self
    }

    pub fn buff_size(mut self, buff_size: usize) -> Self {
        self.buff_size = buff_size;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout.replace(timeout);
        self
    }

    pub fn build(self) -> Result<TcpStream> {
        let Self {
            addr,
            timeout,
            buff_size,
            laddr,
        } = self;
        dial(laddr, addr, timeout, buff_size)
    }
}

#[inline]
fn dial(
    laddr: Option<SocketAddr>,
    addr: SocketAddr,
    timeout: Option<Duration>,
    buff_size: usize,
) -> Result<TcpStream> {
    debug!("begin to dial tcp {}", &addr);

    let stream = {
        let socket = match &addr {
            SocketAddr::V4(_) => {
                socket2::Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?
            }
            SocketAddr::V6(_) => {
                socket2::Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?
            }
        };

        let addr = SockAddr::from(addr);

        if buff_size > 0 {
            socket.set_recv_buffer_size(buff_size)?;
            socket.set_send_buffer_size(buff_size)?;
        }

        socket.set_nodelay(true)?;
        socket.set_keepalive(true)?;

        if let Some(laddr) = laddr {
            // enable reuse when local addr is specified
            socket.set_reuse_address(true)?;
            socket.set_reuse_port(true)?;
            let laddr = SockAddr::from(laddr);
            socket.bind(&laddr)?;
        }

        match timeout {
            Some(t) => socket.connect_timeout(&addr, t)?,
            None => socket.connect(&addr)?,
        }

        let stream: std::net::TcpStream = socket.into();
        stream.set_nonblocking(true)?;

        TcpStream::from_std(stream)
    };

    match stream {
        Ok(c) => {
            debug!("connect ok: {} -> {}", c.local_addr().unwrap(), &addr);
            Ok(c)
        }
        Err(e) => {
            error!("connect {} failed: {}", &addr, e);
            Err(e.into())
        }
    }
}

#[inline]
pub(crate) fn is_health(conn: &TcpStream) -> Result<()> {
    use std::io::ErrorKind::WouldBlock;
    let mut b = [0u8; 0];
    // check if connection is readable
    match conn.try_read(&mut b) {
        Ok(n) => {
            if n == 0 {
                debug!("connection {:?} is closed", conn.local_addr());
            } else {
                warn!(
                    "invalid connection {:?}: should not read any bytes",
                    conn.local_addr()
                );
            }
            Err(CapybaraError::InvalidConnection)
        }
        Err(ref e) if e.kind() == WouldBlock => {
            // check if connection is writeable
            if let Err(e) = conn.try_write(&b[..]) {
                if e.kind() != WouldBlock {
                    debug!(
                        "connection {:?} is not writeable: {:?}",
                        conn.local_addr(),
                        e
                    );
                    return Err(CapybaraError::InvalidConnection);
                }
            }
            Ok(())
        }
        Err(e) => {
            error!("broken connection {:?}: {}", conn.local_addr(), e);
            Err(CapybaraError::InvalidConnection)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &[u8] = b"GET /anything/abc### HTTP/1.1\r\n\
    Host: example.com\r\n\
    User-Agent: capybara/0.1.0\r\n\
    Accept: *\r\n\
    \r\n";

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_tcp_conn() -> anyhow::Result<()> {
        init();

        use bytes::BytesMut;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use crate::resolver::{Resolver, StandardDNSResolver};

        let domain = "httpbin.org";

        let host = {
            let resolver = StandardDNSResolver::default();
            resolver.resolve_one(domain).await?
        };

        let mut stream = TcpStreamBuilder::new(SocketAddr::new(host, 80))
            .buff_size(4096)
            .build()?;
        stream.write_all(B).await?;

        let mut b = BytesMut::with_capacity(8192);
        stream.read_buf(&mut b).await?;
        let bb = b.freeze();

        info!("response: {:?}", bb);

        Ok(())
    }

    #[tokio::test]
    async fn test_tcp_stream_connect_with_laddr() {
        init();

        let to_addr = |s: &str| s.parse::<SocketAddr>().unwrap();

        let laddr = to_addr("0.0.0.0:54321");

        let s1 = {
            let addr = to_addr("223.5.5.5:53");
            TcpStreamBuilder::new(addr).local_addr(laddr).build()
        };
        assert!(s1.is_ok_and(|s1| {
            info!("************ connect ok: {:?}", &s1);
            true
        }));

        let s2 = {
            let addr = to_addr("114.114.114.114:53");
            TcpStreamBuilder::new(addr).local_addr(laddr).build()
        };
        assert!(s2.is_ok_and(|s2| {
            info!("************ connect ok: {:?}", &s2);
            true
        }));
    }
}
