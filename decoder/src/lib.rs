//! Reads a capture file of Honkai: Star Rail logging in (pcap or pcapng, as PCAPdroid on Android, tcpdump or
//! Wireshark save it) and returns the account as Reliquary Archiver's export JSON. Built for the browser
//! (wasm32-wasip1), where the site runs it on the reader's own device: the capture, which holds the login,
//! never leaves it.
//!
//! The live game's traffic is KCP over UDP, encrypted, and goes through reliquary exactly as the desktop app's
//! does. The PearlSR emulator's is plain TCP, read with the same frame reader the app's emulator mode uses.

use std::collections::HashMap;

use reliquary::network::command::command_id::PlayerGetTokenScRsp;
use reliquary::network::command::{GameCommand, GameCommandError};
use reliquary::network::{GamePacket, GameSniffer, NetworkError};
use sync_core::export::Exporter;
use sync_core::export::fribbels::OptimizerExporter;
use sync_core::packets::{EMULATOR_PORT, TcpStream, Transport, ethernet_frame, transport};

mod capture;
use capture::{Packet, ip_packet, read_capture};

#[derive(Default)]
struct Decoder {
    sessions: HashMap<u32, OptimizerExporter>,
    last_login: Option<u32>,
    game_packets: usize,
    key_missing: usize,
}

impl Decoder {
    fn command(&mut self, session: u32, command: GameCommand) {
        if command.command_id == PlayerGetTokenScRsp {
            self.last_login = Some(session);
        }
        self.sessions.entry(session).or_insert_with(OptimizerExporter::new).read_command(command);
    }

    /// The account of the last login that came through whole, or why there is none.
    fn finish(self) -> Result<String, String> {
        let complete = |id: &u32| self.sessions.get(id).filter(|e| e.is_initialized());
        let exporter = self
            .last_login
            .as_ref()
            .and_then(complete)
            .or_else(|| self.sessions.values().find(|e| e.is_initialized()));
        if let Some(export) = exporter.and_then(|e| e.export()) {
            return serde_json::to_string(&export).map_err(|e| e.to_string());
        }
        Err(if self.game_packets == 0 {
            "There is no Honkai: Star Rail traffic in this capture. Capture the game itself, from before you click to start.".into()
        } else if self.last_login.is_none() && self.key_missing > 0 {
            "This capture starts after the game had already logged in. Start capturing first, then open the game and click to start.".into()
        } else {
            "The capture has the game's traffic but not your whole account. Let it run until you are in game before you stop it.".into()
        })
    }
}

/// The account in `capture` as Reliquary Archiver's export JSON, or a sentence saying why it is not there.
pub fn decode(capture: &[u8]) -> Result<String, String> {
    let packets = read_capture(capture)?;
    let mut decoder = Decoder::default();
    let mut sniffer = GameSniffer::new();
    let mut tcp: HashMap<u16, TcpStream> = HashMap::new();

    for Packet { link, data } in packets {
        let Some((ethertype, ip)) = ip_packet(link, data) else { continue };
        match transport(ip) {
            udp if udp.is_game() => {
                decoder.game_packets += 1;
                let packets = match sniffer.receive_packet(ethernet_frame(ethertype, ip)) {
                    Ok(packets) => packets,
                    Err(NetworkError::GameCommand(GameCommandError::DecryptionKeyMissing)) => {
                        decoder.key_missing += 1;
                        continue;
                    }
                    Err(_) => continue,
                };
                for packet in packets {
                    match packet {
                        GamePacket::Commands { conv_id, result: Ok(command) } => decoder.command(conv_id, command),
                        GamePacket::Commands { result: Err(GameCommandError::DecryptionKeyMissing), .. } => decoder.key_missing += 1,
                        _ => {}
                    }
                }
            }
            // Only the server's side carries the account.
            Transport::Tcp { src, dst, seq, syn, payload } if src == EMULATOR_PORT => {
                decoder.game_packets += 1;
                for frame in tcp.entry(dst).or_default().push(seq, syn, payload) {
                    if let Ok(command) = GameCommand::try_new(frame) {
                        decoder.command(0x8000_0000 | u32::from(dst), command);
                    }
                }
            }
            _ => {}
        }
    }
    decoder.finish()
}

/// The browser's entry points. The page copies the capture into memory from `alloc`, calls `decode_capture`,
/// reads back `[ok: u8][length: u32 LE][UTF-8 text]` (the export JSON, or the reason) and frees both buffers.
#[cfg(target_family = "wasm")]
mod wasm {
    #[unsafe(no_mangle)]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        let mut buffer = Vec::<u8>::with_capacity(len);
        let ptr = buffer.as_mut_ptr();
        std::mem::forget(buffer);
        ptr
    }

    /// # Safety
    /// `ptr` and `len` must come from one `alloc` call (or one `decode_capture` result) and be freed once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
        drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
    }

    /// # Safety
    /// `ptr..ptr + len` must be memory the page wrote through `alloc`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn decode_capture(ptr: *const u8, len: usize) -> *mut u8 {
        let capture = unsafe { std::slice::from_raw_parts(ptr, len) };
        let (ok, text) = match super::decode(capture) {
            Ok(json) => (1u8, json),
            Err(reason) => (0u8, reason),
        };
        let mut out = Vec::with_capacity(5 + text.len());
        out.push(ok);
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        let ptr = out.as_mut_ptr();
        std::mem::forget(out);
        ptr
    }
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn a_capture_without_the_game_says_so() {
        // An empty pcap: header only.
        let mut pcap = 0xa1b2_c3d4u32.to_le_bytes().to_vec();
        pcap.extend([2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 1, 0, 0, 0]);
        assert!(decode(&pcap).unwrap_err().contains("no Honkai: Star Rail traffic"));
        assert!(decode(b"not a capture").unwrap_err().contains("not a capture file"));
    }

    /// A real login, recorded from the PearlSR emulator through the app's relay (`--emulator --record`).
    #[test]
    fn an_emulator_login_comes_out_as_the_whole_account() {
        let capture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/emulator-login.pcap")).unwrap();
        let export: serde_json::Value = serde_json::from_str(&decode(&capture).unwrap()).unwrap();
        assert_eq!(export["metadata"]["trailblazer"], "Caelus");
        assert_eq!(export["relics"].as_array().unwrap().len(), 558);
        assert_eq!(export["light_cones"].as_array().unwrap().len(), 93);
        assert_eq!(export["characters"].as_array().unwrap().len(), 93);
    }
}
