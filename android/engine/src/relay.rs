//! The app's own VPN, for the game only: Android hands the game's packets to the VPN interface, a user-space
//! TCP/IP stack (ipstack) ends each of the game's connections here, and the app opens the same connection to
//! the same server and passes the bytes both ways unchanged. The game's own packets are copied to the decoder on
//! the way through; nothing else is read.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use ipstack::{IpStack, IpStackConfig, IpStackStream, IpStackTcpStream, IpStackUdpStream};
use sync_core::packets::{EMULATOR_PORT, Transport, ethernet_frame, transport};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, copy_bidirectional};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Instant, Sleep};
use tracing::debug;

pub const MTU: u16 = 1500;

/// The game's packets: KCP over UDP on its ports, or the PearlSR emulator's plain TCP.
fn is_game(ip: &[u8]) -> bool {
    match transport(ip) {
        Transport::Tcp { src, dst, .. } => src == EMULATOR_PORT || dst == EMULATOR_PORT,
        other => other.is_game(),
    }
}

/// The VPN interface with a tap: every packet through it either way that is the game's goes to the decoder too,
/// as the Ethernet frame it reads.
pub struct Tapped<D> {
    device: D,
    decoder: UnboundedSender<Vec<u8>>,
}

impl<D> Tapped<D> {
    pub fn new(device: D, decoder: UnboundedSender<Vec<u8>>) -> Self {
        Self { device, decoder }
    }

    fn observe(&self, ip: &[u8]) {
        let ethertype = match ip.first().map(|b| b >> 4) {
            Some(4) => 0x0800,
            Some(6) => 0x86dd,
            _ => return,
        };
        if is_game(ip) {
            self.decoder.send(ethernet_frame(ethertype, ip)).ok();
        }
    }
}

impl<D: AsyncRead + Unpin> AsyncRead for Tapped<D> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.device).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            self.observe(&buf.filled()[before..]);
        }
        polled
    }
}

impl<D: AsyncWrite + Unpin> AsyncWrite for Tapped<D> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, packet: &[u8]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.device).poll_write(cx, packet);
        if let Poll::Ready(Ok(written)) = polled {
            self.observe(&packet[..written]);
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.device).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.device).poll_shutdown(cx)
    }
}

/// The VPN interface's file descriptor, one IP packet per read or write.
#[cfg(unix)]
pub mod interface {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::OwnedFd;
    use std::pin::Pin;
    use std::task::{Context, Poll, ready};

    use tokio::io::unix::AsyncFd;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    pub struct Interface(AsyncFd<File>);

    impl Interface {
        /// `fd` must be non-blocking (the app's VPN builder sets it so).
        pub fn new(fd: OwnedFd) -> io::Result<Self> {
            // SAFETY: the File owns the descriptor and nothing else closes or replaces it while the AsyncFd holds it.
            let registered = unsafe { AsyncFd::register(File::from(fd)) };
            Ok(Self(registered.map_err(io::Error::from)?))
        }
    }

    impl AsyncRead for Interface {
        fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            loop {
                let mut ready = ready!(self.0.poll_read_ready(cx))?;
                let unfilled = buf.initialize_unfilled();
                match ready.try_io(|file| file.get_ref().read(unfilled)) {
                    Ok(Ok(read)) => {
                        buf.advance(read);
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(e)) => return Poll::Ready(Err(e)),
                    Err(_would_block) => continue,
                }
            }
        }
    }

    impl AsyncWrite for Interface {
        fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, packet: &[u8]) -> Poll<io::Result<usize>> {
            loop {
                let mut ready = ready!(self.0.poll_write_ready(cx))?;
                match ready.try_io(|file| file.get_ref().write(packet)) {
                    Ok(result) => return Poll::Ready(result),
                    Err(_would_block) => continue,
                }
            }
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}

/// Carries the game's connections to where they were going until the interface closes. The app is not inside its
/// own VPN, so its sockets reach the servers directly.
pub async fn relay(device: impl AsyncRead + AsyncWrite + Unpin + Send + 'static) {
    let mut config = IpStackConfig::default();
    config.mtu_unchecked(MTU);
    let mut stack = IpStack::new(config, device);
    while let Ok(stream) = stack.accept().await {
        match stream {
            IpStackStream::Tcp(stream) => {
                tokio::spawn(async move {
                    let to = stream.peer_addr();
                    let mut game = Closing { stream, retry: Box::pin(tokio::time::sleep(Duration::ZERO)) };
                    match TcpStream::connect(to).await {
                        Ok(mut server) => {
                            copy_bidirectional(&mut game, &mut server).await.ok();
                        }
                        Err(e) => debug!(%to, %e, "could not reach"),
                    }
                    game.shutdown().await.ok();
                });
            }
            IpStackStream::Udp(game) => {
                tokio::spawn(udp(game));
            }
            IpStackStream::UnknownTransport(_) | IpStackStream::UnknownNetwork(_) => {}
        }
    }
}

/// One of the game's TCP connections. ipstack sends its FIN from `poll_shutdown`, and only once the game has
/// acknowledged everything sent, but nothing polls the shutdown again when those acknowledgements arrive: a server
/// that sends a lot and closes would leave the game waiting forever. So a pending shutdown is polled again shortly.
struct Closing {
    stream: IpStackTcpStream,
    retry: Pin<Box<Sleep>>,
}

impl AsyncRead for Closing {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Closing {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.stream).poll_shutdown(cx);
        if polled.is_pending() && this.retry.as_mut().poll(cx).is_ready() {
            this.retry.as_mut().reset(Instant::now() + Duration::from_millis(20));
            cx.waker().wake_by_ref();
        }
        polled
    }
}

/// One of the game's UDP flows (a DNS lookup, the KCP session), datagram for datagram, until ipstack times it out.
async fn udp(mut game: IpStackUdpStream) {
    let to = game.peer_addr();
    let local: SocketAddr = if to.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { ([0u16; 8], 0).into() };
    let Ok(server) = UdpSocket::bind(local).await else { return };
    if server.connect(to).await.is_err() {
        return;
    }
    let (mut up, mut down) = (vec![0; 65535], vec![0; 65535]);
    loop {
        tokio::select! {
            read = game.read(&mut up) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    server.send(&up[..n]).await.ok();
                }
            },
            // A refused datagram (ICMP unreachable) is not the end of the flow.
            received = server.recv(&mut down) => if let Ok(n) = received {
                if game.write_all(&down[..n]).await.is_err() {
                    break;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_game;

    fn ipv4(protocol: u8, src: u16, dst: u16) -> Vec<u8> {
        let mut ip = vec![0x45, 0, 0, 40, 0, 0, 0, 0, 64, protocol, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2];
        ip.extend(src.to_be_bytes());
        ip.extend(dst.to_be_bytes());
        ip.extend([0; 16]);
        ip[32] = 0x50;
        ip
    }

    #[test]
    fn only_the_games_packets_reach_the_decoder() {
        assert!(is_game(&ipv4(17, 51000, 23301)), "KCP to the game server");
        assert!(is_game(&ipv4(17, 23302, 51000)), "KCP from it");
        assert!(is_game(&ipv4(6, 23301, 51000)), "the emulator's TCP");
        assert!(!is_game(&ipv4(6, 51000, 443)), "the game's HTTPS");
        assert!(!is_game(&ipv4(17, 51000, 53)), "a DNS lookup");
    }
}
