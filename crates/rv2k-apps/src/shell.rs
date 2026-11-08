//! Incremental plain TCP command lines. No allocation, terminal protocol or I/O.
pub const CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Ps,
    Mem,
    Uptime,
    Netstat,
    Help,
    Quit,
    Empty,
    Unknown,
    TooLong,
    InvalidInput,
}

// Values above the maximum length encode discard/CRLF state, never indices.
const TOO_LONG: u8 = CAPACITY as u8 + 1;
const INVALID: u8 = TOO_LONG + 1;
const SKIP_LF: u8 = INVALID + 1;
const PS: u64 = u64::from_be_bytes(*b"\0\0\0\0\0\0ps");
const MEM: u64 = u64::from_be_bytes(*b"\0\0\0\0\0mem");
const UPTIME: u64 = u64::from_be_bytes(*b"\0\0uptime");
const NETSTAT: u64 = u64::from_be_bytes(*b"\0netstat");
const HELP: u64 = u64::from_be_bytes(*b"\0\0\0\0help");
const QUIT: u64 = u64::from_be_bytes(*b"\0\0\0\0quit");
const _: () = assert!(CAPACITY <= 252);
const _: () = assert!((PS | MEM | UPTIME | NETSTAT | HELP | QUIT) >> 56 == 0);

pub struct Line {
    word: u64,
    used: u8,
    trailing: bool,
}
impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}
impl Line {
    pub const fn new() -> Self {
        Self {
            word: 0,
            used: 0,
            trailing: false,
        }
    }
    /// Discard a partial line and CRLF state when starting another connection.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    /// Return one command at CR or LF; suppress LF immediately following CR.
    /// After an error, discard the entire suffix through the next terminator.
    #[inline(never)]
    pub fn push(&mut self, byte: u8) -> Option<Command> {
        if self.used == SKIP_LF {
            self.used = 0;
            if byte == b'\n' {
                return None;
            }
        }
        if matches!(byte, b'\r' | b'\n') {
            let command = match self.used {
                TOO_LONG => Command::TooLong,
                INVALID => Command::InvalidInput,
                _ => match self.word {
                    0 => Command::Empty,
                    PS => Command::Ps,
                    MEM => Command::Mem,
                    UPTIME => Command::Uptime,
                    NETSTAT => Command::Netstat,
                    HELP => Command::Help,
                    QUIT => Command::Quit,
                    _ => Command::Unknown,
                },
            };
            self.reset();
            if byte == b'\r' {
                self.used = SKIP_LF;
            }
            return Some(command);
        }
        if self.used < CAPACITY as u8 {
            self.used += 1;
            if !matches!(byte, b'\t' | 0x20..=0x7e) {
                self.used = INVALID;
            } else if matches!(byte, b' ' | b'\t') {
                if self.word != 0 {
                    self.trailing = true;
                }
            } else {
                // Every retained byte is nonzero ASCII. A word of 8+ bytes
                // keeps eight nonzero bytes, so it cannot alias a valid name
                // (all names have at most seven bytes). Internal whitespace
                // also invalidates the name, rather than joining tokens.
                self.word = if self.trailing {
                    u64::MAX
                } else {
                    (self.word << 8) | byte as u64
                };
            }
        } else if self.used == CAPACITY as u8 {
            self.used = TOO_LONG;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{vec, vec::Vec};
    fn collect(line: &mut Line, bytes: &[u8], commands: &mut Vec<Command>) {
        commands.extend(bytes.iter().filter_map(|b| line.push(*b)));
    }
    fn parse(bytes: &[u8], chunk: usize) -> Vec<Command> {
        let mut line = Line::new();
        let mut commands = vec![];
        for part in bytes.chunks(chunk) {
            collect(&mut line, part, &mut commands);
        }
        commands
    }
    #[test]
    fn every_chunk_and_split_preserves_commands_and_crlf() {
        let input = b"ps\r\nmem\nuptime\rnetstat\r\nhelp\nquit\n \t\nPS\nps arg\n";
        let expected = [
            Command::Ps,
            Command::Mem,
            Command::Uptime,
            Command::Netstat,
            Command::Help,
            Command::Quit,
            Command::Empty,
            Command::Unknown,
            Command::Unknown,
        ];
        for chunk in 1..=input.len() {
            assert_eq!(parse(input, chunk), expected);
        }
        for split in 0..=input.len() {
            let mut line = Line::new();
            let mut commands = vec![];
            collect(&mut line, &input[..split], &mut commands);
            collect(&mut line, &input[split..], &mut commands);
            assert_eq!(commands, expected);
        }
        assert_eq!(parse(b"\r\n\n\r\r\n", 1), [Command::Empty; 4]);
    }
    #[test]
    fn exact_limit_overflow_discards_suffix_and_recovers_at_newline() {
        assert!(core::mem::size_of::<Line>() <= 68);
        let mut input = vec![b' '; 62];
        input.extend_from_slice(b"ps\r\n");
        input.extend_from_slice(&[b' '; 63]);
        input.extend_from_slice(b"ps\r\n"); // 65 bytes, including whitespace.
        input.extend_from_slice(&[b'x'; 5000]);
        input.extend_from_slice(b"ps\0\nmem\n"); // Command-looking suffix is discarded.
        for chunk in [1, 3, 64, 128, input.len()] {
            assert_eq!(
                parse(&input, chunk),
                [
                    Command::Ps,
                    Command::TooLong,
                    Command::TooLong,
                    Command::Mem
                ]
            );
        }
    }
    #[test]
    fn invalid_octets_do_not_execute_a_prefix_or_suffix() {
        for byte in 0..=u8::MAX {
            if matches!(byte, b'\r' | b'\n') {
                continue;
            }
            let mut line = Line::new();
            assert_eq!(line.push(byte), None);
            let expected = match byte {
                b' ' | b'\t' => Command::Empty,
                0x21..=0x7e => Command::Unknown,
                _ => Command::InvalidInput,
            };
            assert_eq!(line.push(b'\n'), Some(expected));
        }
        for byte in [0, 8, 27, 127, 255] {
            let mut input = b"ps".to_vec();
            input.push(byte);
            input.extend_from_slice(b"netstat\nmem\n");
            assert_eq!(parse(&input, 1), [Command::InvalidInput, Command::Mem]);
        }
    }
    #[test]
    fn reconnect_reset_drops_partial_overflow_and_pending_crlf() {
        let mut line = Line::new();
        assert_eq!(line.push(b'p'), None);
        line.reset();
        assert_eq!(line.push(b's'), None);
        assert_eq!(line.push(b'\n'), Some(Command::Unknown));
        for _ in 0..100 {
            line.push(b'x');
        }
        line.reset();
        assert_eq!(line.push(b'\r'), Some(Command::Empty));
        line.reset();
        assert_eq!(line.push(b'\n'), Some(Command::Empty));
        let mut commands = vec![];
        collect(&mut line, b"ps\n", &mut commands);
        assert_eq!(commands, [Command::Ps]);
    }
    #[test]
    fn packed_names_match_a_buffered_reference_without_suffix_aliases() {
        fn reference(input: &[u8]) -> Command {
            let mut buffered = Vec::new();
            for &byte in input {
                if buffered.len() == CAPACITY {
                    return Command::TooLong;
                }
                if !matches!(byte, b'\t' | 0x20..=0x7e) {
                    return Command::InvalidInput;
                }
                buffered.push(byte);
            }
            match buffered.trim_ascii() {
                b"ps" => Command::Ps,
                b"mem" => Command::Mem,
                b"uptime" => Command::Uptime,
                b"netstat" => Command::Netstat,
                b"help" => Command::Help,
                b"quit" => Command::Quit,
                b"" => Command::Empty,
                _ => Command::Unknown,
            }
        }
        let check = |mut input: Vec<u8>| {
            let expected = reference(&input);
            input.extend_from_slice(b"\r\n");
            for chunk in [1, 3, input.len()] {
                assert_eq!(parse(&input, chunk), [expected], "{input:?}");
            }
        };
        for name in [
            b"ps".as_slice(),
            b"mem",
            b"uptime",
            b"netstat",
            b"help",
            b"quit",
        ] {
            for position in 0..=name.len() {
                for byte in 0..=u8::MAX {
                    if matches!(byte, b'\r' | b'\n') {
                        continue;
                    }
                    let mut input = name.to_vec();
                    input.insert(position, byte);
                    check(input);
                }
            }
            for count in 0..=CAPACITY + 1 {
                for prefix in [b'x', b'!', b'\t', b' '] {
                    let mut input = vec![prefix; count];
                    input.extend_from_slice(name);
                    check(input);
                }
                let mut input = name.to_vec();
                input.extend_from_slice(&vec![b'\t'; count]);
                check(input);
            }
        }
        assert!(core::mem::size_of::<Line>() <= 16);
    }
}
