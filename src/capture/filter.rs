//! The socket filter the Linux capture hands the kernel: the game's UDP packets, either direction, and the
//! emulator's TCP stream from its server. Everything else is dropped before it reaches the app. Classic BPF,
//! run on the packet from its IP header (a cooked packet socket strips the link header first).

/// One classic BPF instruction: opcode, jump-if-true, jump-if-false, operand.
pub type Insn = (u16, u8, u8, u32);

const LDB_ABS: u16 = 0x30;
const LDH_ABS: u16 = 0x28;
const LDH_IND: u16 = 0x48;
const LDX_IMM: u16 = 0x01;
const LDX_MSH: u16 = 0xb1;
const RSH: u16 = 0x74;
const JEQ: u16 = 0x15;
const JGT: u16 = 0x25;
const JGE: u16 = 0x35;
const JSET: u16 = 0x45;
const RET: u16 = 0x06;

/// The most of one packet kept: more than any IP packet, so nothing is cut.
pub const KEEP: u32 = 262_144;

#[rustfmt::skip]
pub const GAME_FILTER: [Insn; 24] = [
    (LDB_ABS, 0, 0, 0),         //  0  A = the version nibble
    (RSH, 0, 0, 4),             //  1
    (JEQ, 0, 6, 4),             //  2  IPv4? else 9
    (LDH_ABS, 0, 0, 6),         //  3  IPv4: a fragment after the first has no ports
    (JSET, 18, 0, 0x1fff),      //  4  -> drop
    (LDB_ABS, 0, 0, 9),         //  5  A = protocol
    (LDX_MSH, 0, 0, 0),         //  6  X = header length
    (JEQ, 6, 0, 17),            //  7  UDP -> 14
    (JEQ, 11, 14, 6),           //  8  TCP -> 20, else drop
    (JEQ, 0, 13, 6),            //  9  IPv6? else drop
    (LDB_ABS, 0, 0, 6),         // 10  A = next header
    (LDX_IMM, 0, 0, 40),        // 11  X = header length
    (JEQ, 1, 0, 17),            // 12  UDP -> 14
    (JEQ, 6, 9, 6),             // 13  TCP -> 20, else drop
    (LDH_IND, 0, 0, 0),         // 14  UDP: source port a game port?
    (JGE, 0, 1, 23301),         // 15
    (JGT, 0, 5, 23302),         // 16  -> keep
    (LDH_IND, 0, 0, 2),         // 17  destination port a game port?
    (JGE, 0, 4, 23301),         // 18
    (JGT, 3, 2, 23302),         // 19  -> drop, else keep
    (LDH_IND, 0, 0, 0),         // 20  TCP: from the emulator's port?
    (JEQ, 0, 1, 23301),         // 21
    (RET, 0, 0, KEEP),          // 22  keep
    (RET, 0, 0, 0),             // 23  drop
];

#[cfg(test)]
mod tests {
    use super::{GAME_FILTER, Insn, KEEP};

    /// Just enough of a BPF machine to run the filter: what the kernel would return for `packet`.
    fn run(program: &[Insn], packet: &[u8]) -> u32 {
        let (mut a, mut x, mut pc) = (0u32, 0u32, 0usize);
        let byte = |at: usize| packet.get(at).copied().map(u32::from);
        let half = |at: usize| Some((byte(at)? << 8) | byte(at + 1)?);
        loop {
            let (code, jt, jf, k) = program[pc];
            pc += 1;
            let jump = |taken: bool| if taken { usize::from(jt) } else { usize::from(jf) };
            // As in the kernel, a load past the end of the packet drops it.
            match code {
                0x30 => match byte(k as usize) {
                    Some(v) => a = v,
                    None => return 0,
                },
                0x28 => match half(k as usize) {
                    Some(v) => a = v,
                    None => return 0,
                },
                0x48 => match half((x + k) as usize) {
                    Some(v) => a = v,
                    None => return 0,
                },
                0x01 => x = k,
                0xb1 => x = (byte(k as usize).unwrap_or(0) & 0xf) * 4,
                0x74 => a >>= k,
                0x15 => pc += jump(a == k),
                0x25 => pc += jump(a > k),
                0x35 => pc += jump(a >= k),
                0x45 => pc += jump(a & k != 0),
                0x06 => return k,
                other => panic!("opcode {other:#x} not in the test machine"),
            }
        }
    }

    fn ipv4(protocol: u8, fragment: u16, src: u16, dst: u16) -> Vec<u8> {
        let mut ip = vec![0x45, 0, 0, 40, 0, 0];
        ip.extend(fragment.to_be_bytes());
        ip.extend([64, protocol, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2]);
        ip.extend(src.to_be_bytes());
        ip.extend(dst.to_be_bytes());
        ip.extend([0; 16]);
        ip
    }

    fn ipv6(protocol: u8, src: u16, dst: u16) -> Vec<u8> {
        let mut ip = vec![0x60, 0, 0, 0, 0, 20, protocol, 64];
        ip.extend([0; 32]);
        ip.extend(src.to_be_bytes());
        ip.extend(dst.to_be_bytes());
        ip.extend([0; 16]);
        ip
    }

    #[test]
    fn keeps_the_game_and_the_emulators_server_side_and_nothing_else() {
        let keep = |packet: Vec<u8>| run(&GAME_FILTER, &packet) == KEEP;
        // The game, either direction, both ports, both IP versions.
        assert!(keep(ipv4(17, 0, 51000, 23301)));
        assert!(keep(ipv4(17, 0, 23302, 51000)));
        assert!(keep(ipv6(17, 23301, 51000)));
        assert!(keep(ipv6(17, 51000, 23302)));
        // The emulator's server side only.
        assert!(keep(ipv4(6, 0, 23301, 50000)));
        assert!(keep(ipv6(6, 23301, 50000)));
        assert!(!keep(ipv4(6, 0, 50000, 23301)));
        // Everything else.
        assert!(!keep(ipv4(17, 0, 51000, 23300)));
        assert!(!keep(ipv4(17, 0, 23303, 443)));
        assert!(!keep(ipv4(6, 0, 443, 51000)));
        assert!(!keep(ipv4(1, 0, 23301, 23301)));
        assert!(!keep(ipv6(58, 23301, 23301)));
        assert!(!keep(ipv4(17, 0x00b9, 23301, 23301)), "a later fragment's bytes are not ports");
        assert!(!keep(vec![0x20, 0, 0, 0]), "not IP");
        // A first fragment still carries the ports.
        assert!(keep(ipv4(17, 0x2000, 51000, 23301)));
    }
}
