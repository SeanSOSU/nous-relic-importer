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
    use std::collections::HashMap;

    use protobuf::Message;
    use reliquary::network::command::GameCommand;
    use serde_json::Value;
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

    /// The recorded emulator login, through the capture's own path, and what a test needs to carry on as the
    /// server: the bag it sent, and where its stream ends.
    struct Session {
        hub: Hub,
        decoder: Decoder,
        bag: GetBagScRsp,
        template: Vec<u8>,
        seq: u32,
    }

    impl Session {
        fn logged_in() -> Self {
            let pcap = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/decoder/tests/emulator-login.pcap")).unwrap();
            let (hub, mut decoder) = (Hub::new(None), Decoder::default());
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
            let (template, seq) = end.unwrap();
            Session { hub, decoder, bag: bag.unwrap(), template, seq }
        }

        /// `sync` as the server's next message on the same connection, as it comes after a change in game.
        fn send(&mut self, sync: PlayerSyncScNotify) {
            let frame = game_frame(command_id::PlayerSyncScNotify, &sync.write_to_bytes().unwrap());
            self.decoder.frame(&self.hub, next_segment(&self.template, self.seq, &frame));
            self.seq = self.seq.wrapping_add(frame.len() as u32);
        }

        /// The account a page opened now would start from.
        fn account(&self) -> Value {
            serde_json::from_str::<Value>(&self.hub.snapshot().unwrap()).unwrap()["data"].clone()
        }
    }

    /// Who wears each item of an export list, by `_uid`.
    fn wearers(items: &Value) -> HashMap<String, String> {
        let text = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_owned);
        items.as_array().unwrap().iter().map(|item| (text(&item["_uid"]), text(&item["location"]))).collect()
    }

    #[test]
    fn relics_that_change_after_login_reach_open_pages_as_they_happen() {
        let mut session = Session::logged_in();
        let (_, mut pages) = session.hub.subscribe();

        // A relic the account did not have: the server's update after a drop.
        let mut gained = session.bag.relic_list[0].clone();
        (gained.unique_id, gained.level, gained.equip_avatar_id) = (9_000_001, 0, 0);
        let mut sync = PlayerSyncScNotify::new();
        sync.relic_list.push(gained);
        session.send(sync);
        let update = pages.try_recv().expect("the page gets the change, with no new login");
        assert!(update.contains(r#""event":"UpdateRelics""#) && update.contains("9000001"), "{update}");
        assert_eq!(session.hub.summary().unwrap().relics, 559);

        // One the account had, upgraded: changed in place, not added.
        let mut upgraded = session.bag.relic_list[1].clone();
        upgraded.level = 15;
        let mut sync = PlayerSyncScNotify::new();
        sync.relic_list.push(upgraded.clone());
        session.send(sync);
        let update = pages.try_recv().unwrap();
        assert!(update.contains(&upgraded.unique_id.to_string()) && update.contains(r#""level":15"#), "{update}");
        assert!(matches!(*session.hub.status().borrow(), Status::Synced { relics: 559, fresh: true, .. }));
    }

    #[test]
    fn relics_and_light_cones_swapped_between_characters_move_on_open_pages() {
        let mut session = Session::logged_in();
        let (_, mut pages) = session.hub.subscribe();
        let before = session.account();

        // Two characters trade the relics in one slot (a relic id's last digit): an equip swap in game sends both,
        // each with its new wearer.
        let worn: Vec<_> = session.bag.relic_list.iter().filter(|r| r.equip_avatar_id != 0).collect();
        let (a, b) = worn
            .iter()
            .flat_map(|a| worn.iter().map(move |b| (*a, *b)))
            .find(|(a, b)| a.tid % 10 == b.tid % 10 && a.equip_avatar_id != b.equip_avatar_id)
            .map(|(a, b)| (a.clone(), b.clone()))
            .expect("two characters wearing a relic in the same slot");
        let mut sync = PlayerSyncScNotify::new();
        for (relic, wearer) in [(&a, b.equip_avatar_id), (&b, a.equip_avatar_id)] {
            let mut moved = relic.clone();
            moved.equip_avatar_id = wearer;
            sync.relic_list.push(moved);
        }
        session.send(sync);
        let (a_id, b_id) = (a.unique_id.to_string(), b.unique_id.to_string());
        let worn_before = wearers(&before["relics"]);
        assert_ne!(worn_before[&a_id], worn_before[&b_id]);
        let update: Value = serde_json::from_str(&pages.try_recv().expect("the swap reaches the page")).unwrap();
        assert_eq!(update["event"], "UpdateRelics");
        let moved = wearers(&update["data"]);
        assert_eq!((&moved[&a_id], &moved[&b_id]), (&worn_before[&b_id], &worn_before[&a_id]));
        // A page that opens now starts from the swap too, with the same relics.
        let after = session.account();
        let worn_after = wearers(&after["relics"]);
        assert_eq!((&worn_after[&a_id], &worn_after[&b_id]), (&worn_before[&b_id], &worn_before[&a_id]));
        assert_eq!(after["relics"].as_array().unwrap().len(), 558);

        // Light cones the same way.
        let cones: Vec<_> = session.bag.equipment_list.iter().filter(|c| c.equip_avatar_id != 0).collect();
        let (c, d) = (cones[0].clone(), (*cones.iter().find(|x| x.equip_avatar_id != cones[0].equip_avatar_id).unwrap()).clone());
        let mut sync = PlayerSyncScNotify::new();
        for (cone, wearer) in [(&c, d.equip_avatar_id), (&d, c.equip_avatar_id)] {
            let mut moved = cone.clone();
            moved.equip_avatar_id = wearer;
            sync.equipment_list.push(moved);
        }
        session.send(sync);
        let (c_id, d_id) = (c.unique_id.to_string(), d.unique_id.to_string());
        let held_before = wearers(&before["light_cones"]);
        let update: Value = serde_json::from_str(&pages.try_recv().expect("the light cone swap reaches the page")).unwrap();
        assert_eq!(update["event"], "UpdateLightCones");
        let moved = wearers(&update["data"]);
        assert_eq!((&moved[&c_id], &moved[&d_id]), (&held_before[&d_id], &held_before[&c_id]));
        let held_after = wearers(&session.account()["light_cones"]);
        assert_eq!((&held_after[&c_id], &held_after[&d_id]), (&held_before[&d_id], &held_before[&c_id]));
    }

    #[test]
    fn relics_salvaged_and_light_cones_used_up_in_game_leave_open_pages() {
        let mut session = Session::logged_in();
        let (_, mut pages) = session.hub.subscribe();
        let ids = |update: &Value| -> Vec<String> { update["data"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect() };

        // Salvaging: the server's update names the relics that are gone. (This recorded account wears every relic it
        // has, so these come off a character first in game; the update is the same.)
        let salvaged: Vec<u32> = session.bag.relic_list.iter().take(3).map(|r| r.unique_id).collect();
        let mut sync = PlayerSyncScNotify::new();
        sync.del_relic_list = salvaged.clone();
        session.send(sync);
        let update: Value = serde_json::from_str(&pages.try_recv().expect("the salvage reaches the page")).unwrap();
        assert_eq!(update["event"], "DeleteRelics");
        assert_eq!(ids(&update), salvaged.iter().map(u32::to_string).collect::<Vec<_>>());
        // A page that opens now, and the saved account, no longer have them.
        let left = wearers(&session.account()["relics"]);
        assert_eq!(left.len(), 555);
        assert!(salvaged.iter().all(|id| !left.contains_key(&id.to_string())));
        assert!(matches!(*session.hub.status().borrow(), Status::Synced { relics: 555, fresh: true, .. }));

        // A light cone fed to another as experience goes the same way.
        let used = session.bag.equipment_list[0].unique_id;
        let mut sync = PlayerSyncScNotify::new();
        sync.del_equipment_list.push(used);
        session.send(sync);
        let update: Value = serde_json::from_str(&pages.try_recv().expect("the used light cone reaches the page")).unwrap();
        assert_eq!(update["event"], "DeleteLightCones");
        assert_eq!(ids(&update), [used.to_string()]);
        assert!(!wearers(&session.account()["light_cones"]).contains_key(&used.to_string()));
    }
}
