//! `--record <file>` with `--emulator`: writes the relayed emulator session as a pcap capture, the same kind of
//! file a phone's capture app saves, so the capture-file decoder can be tested on a real login without a live
//! account. Each chunk becomes an Ethernet/IPv4/TCP packet between 127.0.0.1 (the client) and 127.0.0.2
//! (PearlSR); checksums are left zero, as captures of loopback traffic carry them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SERVER_PORT: u16 = 23301;
const SYN: u8 = 0x02;
const FIN_ACK: u8 = 0x11;
const PSH_ACK: u8 = 0x18;

pub struct Recorder {
    file: Mutex<BufWriter<File>>,
    // Next sequence number per (connection, from the server).
    seq: Mutex<HashMap<(u32, bool), u32>>,
}

impl Recorder {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let mut file = BufWriter::new(File::create(path)?);
        // pcap 2.4, microsecond timestamps, snaplen 262144, Ethernet.
        file.write_all(&0xa1b2_c3d4u32.to_le_bytes())?;
        file.write_all(&2u16.to_le_bytes())?;
        file.write_all(&4u16.to_le_bytes())?;
        file.write_all(&[0; 8])?;
        file.write_all(&262_144u32.to_le_bytes())?;
        file.write_all(&1u32.to_le_bytes())?;
        Ok(Self { file: Mutex::new(file), seq: Mutex::new(HashMap::new()) })
    }

    fn client_port(id: u32) -> u16 {
        50_000 + (id % 10_000) as u16
    }

    fn packet(&self, id: u32, from_server: bool, flags: u8, payload: &[u8]) {
        let seq = {
            let mut seqs = self.seq.lock().unwrap();
            let next = seqs.entry((id, from_server)).or_insert(1_000);
            let seq = *next;
            *next = next.wrapping_add(payload.len() as u32 + u32::from(flags & (SYN | 0x01) != 0));
            seq
        };
        let (client, server) = ([127, 0, 0, 1], [127, 0, 0, 2]);
        let (src, dst, sport, dport) = if from_server {
            (server, client, SERVER_PORT, Self::client_port(id))
        } else {
            (client, server, Self::client_port(id), SERVER_PORT)
        };
        let mut frame = Vec::with_capacity(54 + payload.len());
        frame.extend([0u8; 12]);
        frame.extend(0x0800u16.to_be_bytes());
        frame.extend([0x45, 0]);
        frame.extend(((20 + 20 + payload.len()) as u16).to_be_bytes());
        frame.extend([0, 0, 0x40, 0, 64, 6, 0, 0]);
        frame.extend(src);
        frame.extend(dst);
        frame.extend(sport.to_be_bytes());
        frame.extend(dport.to_be_bytes());
        frame.extend(seq.to_be_bytes());
        frame.extend([0, 0, 0, 0, 0x50, flags, 0xff, 0xff, 0, 0, 0, 0]);
        frame.extend_from_slice(payload);

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let mut file = self.file.lock().unwrap();
        let header = [
            (now.as_secs() as u32).to_le_bytes(),
            now.subsec_micros().to_le_bytes(),
            (frame.len() as u32).to_le_bytes(),
            (frame.len() as u32).to_le_bytes(),
        ];
        let written = header.iter().try_for_each(|h| file.write_all(h)).and_then(|_| file.write_all(&frame));
        if let Err(e) = written.and_then(|_| file.flush()) {
            tracing::warn!(%e, "could not write the recording");
        }
    }

    pub fn opened(&self, id: u32) {
        self.packet(id, false, SYN, &[]);
        self.packet(id, true, SYN, &[]);
    }

    pub fn data(&self, id: u32, from_server: bool, bytes: &[u8]) {
        // An IPv4 packet carries at most 65,535 bytes.
        for piece in bytes.chunks(60_000) {
            self.packet(id, from_server, PSH_ACK, piece);
        }
    }

    pub fn closed(&self, id: u32) {
        self.packet(id, false, FIN_ACK, &[]);
        self.packet(id, true, FIN_ACK, &[]);
    }
}
