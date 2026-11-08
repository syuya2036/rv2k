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

#[derive(Clone, Copy, PartialEq)]
enum Rejection {
    None,
    TooLong,
    InvalidInput,
}

pub struct Line {
    bytes: [u8; CAPACITY],
    used: u8,
    rejection: Rejection,
    skip_lf: bool,
}
impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}
impl Line {
    pub const fn new() -> Self {
        Self {
            bytes: [0; CAPACITY],
            used: 0,
            rejection: Rejection::None,
            skip_lf: false,
        }
    }
    /// Discard a partial line and CRLF state when starting another connection.
    pub fn reset(&mut self) {
        self.used = 0;
        self.rejection = Rejection::None;
        self.skip_lf = false;
    }
    /// Return one command at CR or LF; suppress LF immediately following CR.
    /// After an error, discard the entire suffix through the next terminator.
    pub fn push(&mut self, byte: u8) -> Option<Command> {
        if self.skip_lf {
            self.skip_lf = false;
            if byte == b'\n' {
                return None;
            }
        }
        if matches!(byte, b'\r' | b'\n') {
            let command = match self.rejection {
                Rejection::TooLong => Command::TooLong,
                Rejection::InvalidInput => Command::InvalidInput,
                Rejection::None => {
                    // Only SP/HTAB can be whitespace: other controls never enter bytes.
                    match self.bytes[..self.used as usize].trim_ascii() {
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
            };
            self.reset();
            self.skip_lf = byte == b'\r';
            return Some(command);
        }
        if self.rejection == Rejection::None {
            self.rejection = if self.used as usize == CAPACITY {
                Rejection::TooLong
            } else if !matches!(byte, b'\t' | 0x20..=0x7e) {
                Rejection::InvalidInput
            } else {
                self.bytes[self.used as usize] = byte;
                self.used += 1;
                Rejection::None
            };
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
}
