//! How long does a TCP handshake take, out of one specific network? That's a ping that works
//! without special privileges, and it says whether a network actually reaches the internet at all.

use crate::net::local_addr;
use crate::types::NetInterface;
use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

const PORT: u16 = 443;
const TIMEOUT: Duration = Duration::from_millis(2000);

/// Well-known, reliable hosts — a few per family, so one being unreachable proves nothing.
const TARGETS_V4: &[&str] = &["1.1.1.1", "8.8.8.8"];
const TARGETS_V6: &[&str] = &["2606:4700:4700::1111", "2001:4860:4860::8888"];

/// A non-blocking socket bound to `local` (and to the device, where the OS lets us), connecting
/// to `remote`.
pub fn bound_socket(local: IpAddr, device: Option<&str>, remote: SocketAddr) -> io::Result<Socket> {
    let domain = if remote.is_ipv4() { Domain::IPV4 } else { Domain::IPV6 };
    let sock = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_nonblocking(true)?;
    sock.bind(&SocketAddr::new(local, 0).into())?;
    if let Some(dev) = device {
        bind_device(&sock, dev, remote.is_ipv4())?;
    }
    match sock.connect(&remote.into()) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.raw_os_error() == Some(EINPROGRESS) => {}
        Err(e) => return Err(e),
    }
    Ok(sock)
}

#[cfg(unix)]
const EINPROGRESS: i32 = libc::EINPROGRESS;
#[cfg(windows)]
const EINPROGRESS: i32 = 10036; // WSAEINPROGRESS

#[cfg(target_os = "macos")]
fn bind_device(sock: &Socket, dev: &str, v4: bool) -> io::Result<()> {
    let c = std::ffi::CString::new(dev).map_err(io::Error::other)?;
    let idx = unsafe { libc::if_nametoindex(c.as_ptr()) };
    let idx = std::num::NonZeroU32::new(idx).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no such interface: {dev}")))?;
    if v4 { sock.bind_device_by_index_v4(Some(idx)) } else { sock.bind_device_by_index_v6(Some(idx)) }
}

#[cfg(target_os = "linux")]
fn bind_device(sock: &Socket, dev: &str, _v4: bool) -> io::Result<()> {
    sock.bind_device(Some(dev.as_bytes()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn bind_device(_: &Socket, _: &str, _: bool) -> io::Result<()> {
    Ok(())
}

/// Handshake time in milliseconds, or `None` if nothing got through.
pub async fn connect_ms(local: IpAddr, device: Option<&str>, remote: SocketAddr) -> Option<f64> {
    let t0 = Instant::now();
    let sock = bound_socket(local, device, remote).ok()?;
    let stream = TcpStream::from_std(sock.into()).ok()?;
    tokio::time::timeout(TIMEOUT, stream.writable()).await.ok()?.ok()?;
    // Writable only means "finished connecting" — check it wasn't a refusal.
    if stream.take_error().ok()?.is_some() {
        return None;
    }
    Some(t0.elapsed().as_secs_f64() * 1000.0)
}

/// The best handshake time out of this network, trying each family it has an address for.
pub async fn ping(iface: &NetInterface, bind_to_device: bool) -> Option<f64> {
    let mut v4: Vec<SocketAddr> = TARGETS_V4.iter().filter_map(|t| t.parse().ok().map(|ip| SocketAddr::new(ip, PORT))).collect();
    let v6: Vec<SocketAddr> = TARGETS_V6.iter().filter_map(|t| t.parse().ok().map(|ip| SocketAddr::new(ip, PORT))).collect();
    v4.extend(v6);
    ping_targets(iface, bind_to_device, &v4).await
}

/// Same, against specific targets (the engine can be pointed somewhere else for tests).
pub async fn ping_targets(iface: &NetInterface, bind_to_device: bool, targets: &[SocketAddr]) -> Option<f64> {
    let device = bind_to_device.then_some(iface.id.as_str());
    let mut best: Option<f64> = None;
    for want_v6 in [false, true] {
        let Some(local) = local_addr(iface, want_v6).filter(|a| a.is_ipv6() == want_v6) else { continue };
        for t in targets.iter().filter(|t| t.is_ipv6() == want_v6) {
            if let Some(ms) = connect_ms(local, device, *t).await {
                best = Some(best.map_or(ms, |b| b.min(ms)));
                break;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn measures_a_local_handshake_and_notices_a_dead_port() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let _ = l.accept().await;
            }
        });
        let ms = connect_ms("127.0.0.1".parse().unwrap(), None, addr).await.expect("should connect");
        assert!(ms < 500.0, "{ms}");
        // Nothing listens here.
        let dead: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert!(connect_ms("127.0.0.1".parse().unwrap(), None, dead).await.is_none());
    }
}
