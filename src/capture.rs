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
