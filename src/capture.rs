//! Reads the game's traffic as the platform captures it, Windows' built-in packet monitor (pktmon, so there is
//! no driver to install) or a Linux packet socket, and feeds it through Reliquary's decoder into the
//! archiver's exporter. Adapted from Reliquary Archiver's capture and worker modules (MIT,
//! https://github.com/IceDynamix/reliquary-archiver).

use std::collections::HashMap;
#[cfg(windows)]
use std::time::Duration;

use futures::{Stream, StreamExt};
use reliquary::network::command::{GameCommand, GameCommandError};
use reliquary::network::{ConnectionPacket, GamePacket, GameSniffer, NetworkError};
use sync_core::packets::{EMULATOR_PORT, TcpStream, Transport, ip_in_ethernet, transport};
use tracing::{debug, info, warn};

use crate::accounts::Accounts;
use crate::hub::{Hub, Status};

#[cfg(any(target_os = "linux", test))]
mod filter;
#[cfg(target_os = "linux")]
pub mod packet_socket;
#[cfg(windows)]
mod pktmon;

/// Everything one capture has seen: the live game's KCP sessions, and the emulator's TCP streams.
#[derive(Default)]
struct Decoder {
    sniffer: GameSniffer,
    accounts: Accounts,
    tcp: HashMap<u16, TcpStream>,
}

impl Decoder {
    /// Takes one captured Ethernet frame.
    fn frame(&mut self, hub: &Hub, frame: Vec<u8>) {
        let game = match ip_in_ethernet(&frame).map(|(_, ip)| transport(ip)) {
            // Only the server's side carries the account.
            Some(Transport::Tcp { src: EMULATOR_PORT, dst, seq, syn, payload }) => {
                hub.game_seen();
                let session = 0x8000_0000 | u32::from(dst);
                for frame in self.tcp.entry(dst).or_default().push(seq, syn, payload) {
                    if let Ok(command) = GameCommand::try_new(frame) {
                        self.accounts.command(hub, session, command);
                    }
                }
                false
            }
            Some(transport) => transport.is_game(),
            None => false,
        };
        if game {
            hub.game_seen();
            self.game(hub, frame);
        }
    }

    fn key_missing(&self, hub: &Hub) {
        if !self.accounts.has_login() {
            hub.set_status(Status::NeedsRelogin);
        }
    }

    fn game(&mut self, hub: &Hub, frame: Vec<u8>) {
        let packets = match self.sniffer.receive_packet(frame) {
            Ok(packets) => packets,
            Err(NetworkError::GameCommand(GameCommandError::DecryptionKeyMissing)) => return self.key_missing(hub),
            Err(_) => return,
        };
        for packet in packets {
            let (conv_id, result) = match packet {
                GamePacket::Connection(ConnectionPacket::HandshakeEstablished { conv_id }) => {
                    info!(conv_id, "game connected");
                    if !hub.is_synced() {
                        hub.set_status(Status::GameFound);
                    }
                    continue;
                }
                GamePacket::Connection(ConnectionPacket::Disconnected) => {
                    info!("game disconnected");
                    if !hub.is_synced() {
                        hub.set_status(Status::WaitingForGame);
                    }
                    continue;
                }
                GamePacket::Connection(_) => continue,
                GamePacket::Commands { conv_id, result } => (conv_id, result),
            };
            match result {
                Ok(command) => self.accounts.command(hub, conv_id, command),
                Err(GameCommandError::DecryptionKeyMissing) => self.key_missing(hub),
                Err(e) => debug!(conv_id, %e, "undecodable command"),
            }
        }
    }

    async fn read(&mut self, hub: &Hub, mut frames: impl Stream<Item = Vec<u8>> + Unpin) {
        if !hub.is_synced() {
            hub.set_status(Status::WaitingForGame);
        }
        info!("capturing");
        while let Some(frame) = frames.next().await {
            self.frame(hub, frame);
        }
    }
}

#[cfg(windows)]
pub async fn run(hub: Hub) {
    let mut decoder = Decoder::default();
    loop {
        match pktmon::start() {
            Ok(frames) => decoder.read(&hub, frames).await,
            Err(e) => {
                warn!(%e, "packet capture did not start");
                hub.set_status(Status::Failed(e));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        warn!("capture ended; restarting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Reads the packet socket's frames until it fails. It cannot be reopened: the capability that opened it is
/// gone by then.
#[cfg(target_os = "linux")]
pub async fn run(hub: Hub, frames: impl Stream<Item = Vec<u8>> + Unpin) {
    Decoder::default().read(&hub, frames).await;
    warn!("capture ended");
    hub.set_status(Status::Failed("Packet capture stopped".into()));
}

#[cfg(test)]
mod tests {
    use protobuf::Message;
    use reliquary::network::command::GameCommand;
    use reliquary::network::command::command_id;
    use reliquary::network::command::proto::GetBagScRsp::GetBagScRsp;
    use reliquary::network::command::proto::PlayerSyncScNotify::PlayerSyncScNotify;
    use sync_core::frames::{HEAD_MAGIC, TAIL_MAGIC};
    use sync_core::packets::{EMULATOR_PORT, TcpStream, Transport, ip_in_ethernet, transport};

    use super::Decoder;
    use crate::hub::{Hub, Status};

    /// The frames of a little-endian microsecond pcap, as the recorder writes them.
    fn pcap_frames(file: &[u8]) -> Vec<&[u8]> {
        let (mut frames, mut at) = (Vec::new(), 24);
        while at + 16 <= file.len() {
            let len = u32::from_le_bytes(file[at + 8..at + 12].try_into().unwrap()) as usize;
            frames.push(&file[at + 16..at + 16 + len]);
            at += 16 + len;
        }
        frames
    }

    fn game_frame(command: u16, body: &[u8]) -> Vec<u8> {
        let mut frame = HEAD_MAGIC.to_vec();
        frame.extend(command.to_be_bytes());
        frame.extend(0u16.to_be_bytes());
        frame.extend((body.len() as u32).to_be_bytes());
        frame.extend(body);
        frame.extend(TAIL_MAGIC);
        frame
    }

    /// The server's next TCP segment on `template`'s connection, carrying `payload` from `seq`.
    fn next_segment(template: &[u8], seq: u32, payload: &[u8]) -> Vec<u8> {
        let ip_at = template.len() - ip_in_ethernet(template).unwrap().1.len();
        let ip_header = usize::from(template[ip_at] & 0x0f) * 4;
        let tcp_at = ip_at + ip_header;
        let tcp_header = usize::from(template[tcp_at + 12] >> 4) * 4;
        let mut segment = template[..tcp_at + tcp_header].to_vec();
        segment[ip_at + 2..ip_at + 4].copy_from_slice(&((ip_header + tcp_header + payload.len()) as u16).to_be_bytes());
        segment[tcp_at + 4..tcp_at + 8].copy_from_slice(&seq.to_be_bytes());
        segment.extend(payload);
        segment
    }

    #[test]
    fn relics_that_change_after_login_reach_open_pages_as_they_happen() {
        let pcap = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/decoder/tests/emulator-login.pcap")).unwrap();
        let (hub, mut decoder) = (Hub::new(None), Decoder::default());
        // The recorded login, through the capture's own path; alongside, a relic from the bag the server sent,
        // and where the server's stream ends.
        let (mut stream, mut bag, mut end) = (TcpStream::default(), None, None);
        for frame in pcap_frames(&pcap) {
            if let Some(Transport::Tcp { src: EMULATOR_PORT, seq, syn, payload, .. }) = ip_in_ethernet(frame).map(|(_, ip)| transport(ip)) {
                if !payload.is_empty() {
                    end = Some((frame.to_vec(), seq.wrapping_add(payload.len() as u32)));
                }
                for command in stream.push(seq, syn, payload).into_iter().filter_map(|f| GameCommand::try_new(f).ok()) {
                    if command.command_id == command_id::GetBagScRsp {
                        bag = command.parse_proto::<GetBagScRsp>().ok();
                    }
                }
            }
            decoder.frame(&hub, frame.to_vec());
        }
        assert!(matches!(*hub.status().borrow(), Status::Synced { relics: 558, fresh: true, .. }));
        let (bag, (template, mut seq)) = (bag.unwrap(), end.unwrap());
        let (_, mut pages) = hub.subscribe();
        let mut send = |sync: PlayerSyncScNotify| {
            let frame = game_frame(command_id::PlayerSyncScNotify, &sync.write_to_bytes().unwrap());
            decoder.frame(&hub, next_segment(&template, seq, &frame));
            seq = seq.wrapping_add(frame.len() as u32);
        };

        // A relic the account did not have: the server's update after a drop.
        let mut gained = bag.relic_list[0].clone();
        (gained.unique_id, gained.level, gained.equip_avatar_id) = (9_000_001, 0, 0);
        let mut sync = PlayerSyncScNotify::new();
        sync.relic_list.push(gained);
        send(sync);
        let update = pages.try_recv().expect("the page gets the change, with no new login");
        assert!(update.contains(r#""event":"UpdateRelics""#) && update.contains("9000001"), "{update}");
        assert_eq!(hub.summary().unwrap().relics, 559);

        // One the account had, upgraded: changed in place, not added.
        let mut upgraded = bag.relic_list[1].clone();
        upgraded.level = 15;
        let mut sync = PlayerSyncScNotify::new();
        sync.relic_list.push(upgraded.clone());
        send(sync);
        let update = pages.try_recv().unwrap();
        assert!(update.contains(&upgraded.unique_id.to_string()) && update.contains(r#""level":15"#), "{update}");
        assert_eq!(hub.summary().unwrap().relics, 559);
        assert!(matches!(*hub.status().borrow(), Status::Synced { relics: 559, fresh: true, .. }));
    }
}
