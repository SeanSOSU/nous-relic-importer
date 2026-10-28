//! The Android app's packet source: PCAPdroid's TCP exporter, which sends each capture as a PCAP file over one
//! connection (pcap-over-IP). PCAPdroid captures through Android's VPN service, so its packets are bare IP.

use std::pin::pin;
use std::time::Duration;

use futures::{StreamExt, stream};
use sync_core::packets::{ethernet_frame, ip_packet};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::decoder::Decoder;
use crate::hub::Hub;

/// Larger than any packet a phone's capture holds; a record claiming more means the stream is not PCAP.
const MAX_PACKET: usize = 1 << 18;

fn u32_at(bytes: &[u8], at: usize, big: bool) -> u32 {
    let b: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
    if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
}

/// A PCAP byte stream cut into its packets as the bytes arrive, each as the Ethernet frame the decoder reads.
#[derive(Default)]
pub struct PcapStream {
    buffer: Vec<u8>,
    /// Byte order and link type, from the file header.
    header: Option<(bool, u32)>,
}

impl PcapStream {
    /// Takes the next bytes and returns the frames they complete, or why the stream cannot be read.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut at = 0;
        let (big, link) = match self.header {
            Some(header) => header,
            None if self.buffer.len() < 24 => return Ok(Vec::new()),
            None => {
                let big = match self.buffer[..4] {
                    [0xd4, 0xc3, 0xb2, 0xa1] | [0x4d, 0x3c, 0xb2, 0xa1] => false,
                    [0xa1, 0xb2, 0xc3, 0xd4] | [0xa1, 0xb2, 0x3c, 0x4d] => true,
                    _ => return Err("the capture is not a PCAP stream: set PCAPdroid's dump format to PCAP".into()),
                };
                at = 24;
                *self.header.insert((big, u32_at(&self.buffer, 20, big) & 0x0fff_ffff))
            }
        };
        let mut frames = Vec::new();
        while self.buffer.len() >= at + 16 {
            let captured = u32_at(&self.buffer, at + 8, big) as usize;
            if captured > MAX_PACKET {
                return Err(format!("a {captured}-byte packet: the capture stream is corrupt"));
            }
            let Some(data) = self.buffer.get(at + 16..at + 16 + captured) else { break };
            if let Some((ethertype, ip)) = ip_packet(link, data) {
                frames.push(ethernet_frame(ethertype, ip));
            }
            at += 16 + captured;
        }
        self.buffer.drain(..at);
        Ok(frames)
    }
}

/// Reads every capture PCAPdroid sends to `listener`, one connection at a time and all through one decoder.
/// `capturing` says whether one is coming in now.
pub async fn collect(hub: Hub, listener: TcpListener, capturing: watch::Sender<bool>) {
    let mut decoder = Decoder::default();
    loop {
        let socket = match listener.accept().await {
            Ok((socket, _)) => socket,
            Err(e) => {
                warn!(%e, "could not take a capture connection");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        info!("capture connected");
        capturing.send_replace(true);
        let frames = stream::unfold((socket, PcapStream::default(), vec![0; 1 << 16]), |(mut socket, mut pcap, mut chunk)| async move {
            let read = socket.read(&mut chunk).await.ok().filter(|&n| n > 0)?;
            let frames = pcap.push(&chunk[..read]).inspect_err(|e| warn!(%e, "capture dropped")).ok()?;
            Some((stream::iter(frames), (socket, pcap, chunk)))
        });
        decoder.read(&hub, pin!(frames.flatten())).await;
        capturing.send_replace(false);
        info!("capture ended");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sync_core::packets::ip_in_ethernet;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::watch;

    use super::{PcapStream, collect};
    use crate::hub::{Hub, Status};

    /// The recorded emulator login as PCAPdroid would send it: bare IP packets, link type 101.
    fn as_pcapdroid() -> Vec<u8> {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../decoder/tests/emulator-login.pcap")).unwrap();
        let mut out = file[..24].to_vec();
        out[20..24].copy_from_slice(&101u32.to_le_bytes());
        let mut at = 24;
        while at + 16 <= file.len() {
            let len = u32::from_le_bytes(file[at + 8..at + 12].try_into().unwrap()) as usize;
            let (_, ip) = ip_in_ethernet(&file[at + 16..at + 16 + len]).unwrap();
            out.extend(&file[at..at + 8]);
            out.extend((ip.len() as u32).to_le_bytes());
            out.extend((ip.len() as u32).to_le_bytes());
            out.extend(ip);
            at += 16 + len;
        }
        out
    }

    #[test]
    fn a_stream_cut_anywhere_gives_the_same_packets_as_whole() {
        let capture = as_pcapdroid();
        let whole = PcapStream::default().push(&capture).unwrap();
        assert!(whole.len() > 100);
        let mut stream = PcapStream::default();
        let pieces: Vec<Vec<u8>> = capture.chunks(997).flat_map(|piece| stream.push(piece).unwrap()).collect();
        assert_eq!(pieces, whole);
        assert!(PcapStream::default().push(&[b'x'; 40]).is_err(), "anything but PCAP is refused, not guessed at");
    }

    #[tokio::test]
    async fn a_login_sent_by_pcapdroid_comes_out_as_the_whole_account() {
        let hub = Hub::new(None);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (capturing, mut watching) = watch::channel(false);
        tokio::spawn(collect(hub.clone(), listener, capturing));

        let mut pcapdroid = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        for piece in as_pcapdroid().chunks(1400) {
            pcapdroid.write_all(piece).await.unwrap();
        }
        let mut status = hub.status();
        let synced = tokio::time::timeout(Duration::from_secs(30), status.wait_for(|s| matches!(s, Status::Synced { .. })));
        assert!(matches!(*synced.await.unwrap().unwrap(), Status::Synced { relics: 558, fresh: true, .. }));
        assert!(*watching.borrow_and_update());

        drop(pcapdroid);
        tokio::time::timeout(Duration::from_secs(5), watching.wait_for(|on| !on)).await.unwrap().unwrap();
        assert!(hub.is_synced(), "the account stays once the capture stops");
    }
}
