//! A Linux packet socket on every interface, cut down in the kernel to the game's packets by a socket filter,
//! so nothing else reaches the app. Packets land in a ring shared with the kernel, large enough to hold a
//! whole login even if the app is slow to read it: a packet missed here is gone, since the game itself got it.
//!
//! Opening one needs CAP_NET_RAW, which the installed copy is given once, at install. The app opens it before
//! anything else and then gives the capability up.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{Ordering, fence};

use futures::Stream;
use sync_core::packets::ethernet_frame;
use tracing::{info, warn};

use super::filter::{GAME_FILTER, Insn};

const BLOCK_SIZE: usize = 1 << 20;
const BLOCKS: usize = 4;
const FRAME_SIZE: u32 = 2048;
/// A block that has packets but is not full goes to the app after this long.
const BLOCK_TIMEOUT_MS: u32 = 50;

const TPACKET_V3: libc::c_int = 2;
const TP_STATUS_USER: u32 = 1;
const TP_STATUS_KERNEL: u32 = 0;
const PACKET_OUTGOING: u8 = 4;
const ARPHRD_LOOPBACK: u16 = 772;

#[repr(C)]
struct TpacketReq3 {
    block_size: u32,
    block_nr: u32,
    frame_size: u32,
    frame_nr: u32,
    retire_blk_tov: u32,
    sizeof_priv: u32,
    feature_req_word: u32,
}

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

pub struct PacketSocket {
    fd: OwnedFd,
    ring: *mut u8,
}

// The ring is only touched by the one thread that reads it.
unsafe impl Send for PacketSocket {}

fn check(result: libc::c_int) -> io::Result<()> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn set_option<T>(fd: &OwnedFd, level: libc::c_int, name: libc::c_int, value: &T) -> io::Result<()> {
    let size = size_of::<T>() as libc::socklen_t;
    check(unsafe { libc::setsockopt(fd.as_raw_fd(), level, name, (value as *const T).cast(), size) })
}

impl PacketSocket {
    /// Fails with `PermissionDenied` without CAP_NET_RAW.
    pub fn open() -> io::Result<Self> {
        let protocol = (libc::ETH_P_ALL as u16).to_be() as libc::c_int;
        let raw = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, protocol) };
        check(raw)?;
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let program: Vec<SockFilter> = GAME_FILTER.iter().map(|&(code, jt, jf, k): &Insn| SockFilter { code, jt, jf, k }).collect();
        let fprog = SockFprog { len: program.len() as u16, filter: program.as_ptr() };
        set_option(&fd, libc::SOL_SOCKET, libc::SO_ATTACH_FILTER, &fprog)?;

        set_option(&fd, libc::SOL_PACKET, libc::PACKET_VERSION, &TPACKET_V3)?;
        let request = TpacketReq3 {
            block_size: BLOCK_SIZE as u32,
            block_nr: BLOCKS as u32,
            frame_size: FRAME_SIZE,
            frame_nr: (BLOCK_SIZE as u32 / FRAME_SIZE) * BLOCKS as u32,
            retire_blk_tov: BLOCK_TIMEOUT_MS,
            sizeof_priv: 0,
            feature_req_word: 0,
        };
        set_option(&fd, libc::SOL_PACKET, libc::PACKET_RX_RING, &request)?;
        let ring = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                BLOCK_SIZE * BLOCKS,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ring == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, ring: ring.cast() })
    }

    /// The game's packets as Ethernet frames (what reliquary reads), from a thread of their own that keeps
    /// the priority the app started with: the rest of the app runs below normal, but this must keep up.
    pub fn frames(self) -> impl Stream<Item = Vec<u8>> + Unpin + Send {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || self.read(tx))
            .expect("starting the capture thread");
        Box::pin(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) }))
    }

    fn read(self, tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
        info!("capturing on every interface");
        let mut block = 0;
        loop {
            let desc = unsafe { self.ring.add(block * BLOCK_SIZE) };
            let status = unsafe { desc.add(8).cast::<u32>().read_volatile() };
            if status & TP_STATUS_USER == 0 {
                let mut poll = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN | libc::POLLERR, revents: 0 };
                if unsafe { libc::poll(&mut poll, 1, -1) } < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    warn!(error = %io::Error::last_os_error(), "packet capture failed");
                    return;
                }
                continue;
            }
            fence(Ordering::Acquire);
            let (packets, first) = unsafe { (desc.add(12).cast::<u32>().read(), desc.add(16).cast::<u32>().read()) };
            let mut at = first as usize;
            for _ in 0..packets {
                let header = unsafe { desc.add(at) };
                let (next, snaplen, net) = unsafe {
                    (header.cast::<u32>().read(), header.add(12).cast::<u32>().read(), header.add(26).cast::<u16>().read())
                };
                // The address follows the 48-byte packet header.
                let (hatype, pkttype) = unsafe { (header.add(48 + 8).cast::<u16>().read(), header.add(48 + 10).read()) };
                // Loopback shows each packet twice, leaving and arriving.
                if !(pkttype == PACKET_OUTGOING && hatype == ARPHRD_LOOPBACK) {
                    let ip = unsafe { std::slice::from_raw_parts(header.add(usize::from(net)), snaplen as usize) };
                    let ethertype = if ip.first().is_some_and(|b| b >> 4 == 6) { 0x86dd } else { 0x0800 };
                    if tx.send(ethernet_frame(ethertype, ip)).is_err() {
                        return;
                    }
                }
                at += next as usize;
            }
            fence(Ordering::Release);
            unsafe { desc.add(8).cast::<u32>().write_volatile(TP_STATUS_KERNEL) };
            block = (block + 1) % BLOCKS;
            self.report_drops();
        }
    }

    /// Logs packets the kernel had to drop because the ring was full (reading the count resets it).
    fn report_drops(&self) {
        let mut stats = [0u32; 3];
        let mut size = size_of_val(&stats) as libc::socklen_t;
        let read = unsafe {
            libc::getsockopt(self.fd.as_raw_fd(), libc::SOL_PACKET, libc::PACKET_STATISTICS, stats.as_mut_ptr().cast(), &mut size)
        };
        if read == 0 && stats[1] > 0 {
            warn!(dropped = stats[1], of = stats[0], "the capture ring overflowed; a login may come through incomplete");
        }
    }
}

impl Drop for PacketSocket {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ring.cast(), BLOCK_SIZE * BLOCKS) };
    }
}

#[cfg(test)]
mod tests {
    use super::PacketSocket;
    use futures::StreamExt;
    use std::net::UdpSocket;
    use std::time::Duration;

    /// Needs CAP_NET_RAW (root in a container): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn only_the_games_packets_come_through() {
        let socket = PacketSocket::open().expect("CAP_NET_RAW");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut frames = socket.frames();
            let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
            sender.send_to(b"not the game", "127.0.0.1:9").unwrap();
            sender.send_to(b"the game", "127.0.0.1:23301").unwrap();
            let frame = tokio::time::timeout(Duration::from_secs(2), frames.next()).await.unwrap().unwrap();
            assert!(frame.ends_with(b"the game"), "the first packet through is the game's");
            assert_eq!(&frame[12..14], &[0x08, 0x00]);
            let more = tokio::time::timeout(Duration::from_millis(300), frames.next()).await;
            assert!(more.is_err(), "each loopback packet comes through once");
        });
    }
}
