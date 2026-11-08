//! Bounded POST /echo profile: strict CRLF, one Host and Content-Length,
//! no transfer coding, one request per connection. Headers share the body RAM.
pub const CAPACITY: usize = 64;
pub const MAX_HEADERS: u8 = 16;
pub const MAX_HEAD_BYTES: u16 = 1024;
pub const IO_CAPACITY: usize = 128;
const HOST: u8 = 1;
const LENGTH: u8 = 2;
const CR: u8 = 4;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Ok,
    BadRequest,
    Timeout,
    LengthRequired,
    TooLarge,
    UriTooLong,
    ExpectationFailed,
    HeadersTooLarge,
}
impl Status {
    pub const fn code(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::BadRequest => 400,
            Self::Timeout => 408,
            Self::LengthRequired => 411,
            Self::TooLarge => 413,
            Self::UriTooLong => 414,
            Self::ExpectationFailed => 417,
            Self::HeadersTooLarge => 431,
        }
    }
}
/// Fixed length-delimited header; numeric status and decimal Content-Length
/// carry the response semantics. Leading-zero lengths include 00 for empty.
pub fn response_header(status: Status, length: u8, out: &mut [u8; IO_CAPACITY]) -> usize {
    const TEMPLATE: &[u8] = b"HTTP/1.1 000 Response\r\nContent-Length: 00\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
    assert!(length as usize <= CAPACITY);
    out[..TEMPLATE.len()].copy_from_slice(TEMPLATE);
    const CODES: [[u8; 3]; 8] = [
        *b"200", *b"400", *b"408", *b"411", *b"413", *b"414", *b"417", *b"431",
    ];
    out[9..12].copy_from_slice(&CODES[status as usize]);
    // At most six subtractions; RV32EC has no integer division instruction.
    let mut units = length;
    let mut tens = b'0';
    while units >= 10 {
        units -= 10;
        tens += 1;
    }
    out[39] = tens;
    out[40] = b'0' + units;
    TEMPLATE.len()
}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Line,
    Headers,
    Body,
    Done,
}
pub struct Request {
    bytes: [u8; CAPACITY],
    total: u16,
    used: u8,
    length: u8,
    flags: u8,
    headers: u8,
    colon: u8,
    phase: Phase,
    reply: Status,
}
impl Default for Request {
    fn default() -> Self {
        Self::new()
    }
}
impl Request {
    pub const fn new() -> Self {
        Self {
            bytes: [0; CAPACITY],
            total: 0,
            used: 0,
            length: 0,
            flags: 0,
            headers: 0,
            colon: 0,
            phase: Phase::Line,
            reply: Status::Ok,
        }
    }
    pub fn reset(&mut self) {
        self.total = 0;
        self.used = 0;
        self.length = 0;
        self.flags = 0;
        self.headers = 0;
        self.colon = 0;
        self.phase = Phase::Line;
        self.reply = Status::Ok;
    }
    pub fn body(&self) -> &[u8] {
        if self.phase == Phase::Done && self.reply == Status::Ok {
            &self.bytes[..self.used as usize]
        } else {
            &[]
        }
    }
    fn finish(&mut self, status: Status) -> Option<Status> {
        self.phase = Phase::Done;
        self.reply = status;
        Some(status)
    }
    pub fn timeout(&mut self) -> Status {
        if self.phase != Phase::Done {
            self.finish(Status::Timeout);
        }
        self.reply
    }
    pub fn eof(&mut self) -> Status {
        if self.phase != Phase::Done {
            self.finish(Status::BadRequest);
        }
        self.reply
    }
    /// Stop at the first complete request; trailing octets belong to an
    /// unsupported subsequent request and are never included in this body.
    pub fn feed(&mut self, input: &[u8]) -> Option<Status> {
        if self.phase == Phase::Done {
            return Some(self.reply);
        }
        for &byte in input {
            if self.phase == Phase::Body {
                self.bytes[self.used as usize] = byte;
                self.used += 1;
                if self.used == self.length {
                    return self.finish(Status::Ok);
                }
                continue;
            }
            self.total += 1;
            if self.total > MAX_HEAD_BYTES {
                return self.finish(Status::HeadersTooLarge);
            }
            if self.flags & CR != 0 {
                if byte != b'\n' {
                    return self.finish(Status::BadRequest);
                }
                self.flags &= !CR;
                if let Some(reply) = self.line() {
                    return Some(reply);
                }
            } else if byte == b'\r' {
                self.flags |= CR;
            } else if byte == b'\n' {
                return self.finish(Status::BadRequest);
            } else if self.used as usize == CAPACITY {
                return self.finish(if self.phase == Phase::Line {
                    Status::UriTooLong
                } else {
                    Status::HeadersTooLarge
                });
            } else {
                let byte = if self.phase == Phase::Headers {
                    if self.colon == 0 {
                        if byte == b':' {
                            if self.used == 0 {
                                return self.finish(Status::BadRequest);
                            }
                            self.colon = self.used;
                            byte
                        } else {
                            if !token(byte) {
                                return self.finish(Status::BadRequest);
                            }
                            byte.to_ascii_lowercase()
                        }
                    } else {
                        if !matches!(byte, b'\t' | 0x20..=0x7e) {
                            return self.finish(Status::BadRequest);
                        }
                        byte
                    }
                } else {
                    byte
                };
                self.bytes[self.used as usize] = byte;
                self.used += 1;
            }
        }
        None
    }
    #[inline(never)]
    fn line(&mut self) -> Option<Status> {
        let len = self.used as usize;
        let colon = self.colon as usize;
        self.colon = 0;
        self.used = 0;
        let line = &self.bytes[..len];
        if self.phase == Phase::Line {
            if line != b"POST /echo HTTP/1.1" {
                return self.finish(Status::BadRequest);
            }
            self.phase = Phase::Headers;
            return None;
        }
        if line.is_empty() {
            if self.flags & HOST == 0 {
                return self.finish(Status::BadRequest);
            }
            if self.flags & LENGTH == 0 {
                return self.finish(Status::LengthRequired);
            }
            if self.length == 0 {
                return self.finish(Status::Ok);
            }
            self.phase = Phase::Body;
            return None;
        }
        self.headers += 1;
        if self.headers > MAX_HEADERS {
            return self.finish(Status::HeadersTooLarge);
        }
        if colon == 0 {
            return self.finish(Status::BadRequest);
        }
        let name = &line[..colon];
        let raw = &line[colon + 1..];
        let value = raw.trim_ascii(); // Other ASCII whitespace was rejected above.
        if name == b"content-length" {
            if self.flags & LENGTH != 0 || value.is_empty() || !value.iter().all(u8::is_ascii_digit)
            {
                return self.finish(Status::BadRequest);
            }
            let mut length = 0u16;
            for byte in value {
                // The preceding step was <=64, so the next decimal step fits u16.
                length = length * 10 + (byte - b'0') as u16;
                if length > CAPACITY as u16 {
                    return self.finish(Status::TooLarge);
                }
            }
            self.length = length as u8;
            self.flags |= LENGTH;
        } else if name == b"host" {
            if self.flags & HOST != 0 || !host(value) {
                return self.finish(Status::BadRequest);
            }
            self.flags |= HOST;
        } else if name == b"transfer-encoding" {
            return self.finish(Status::BadRequest);
        } else if name == b"expect" {
            return self.finish(Status::ExpectationFailed);
        }
        None
    }
}
fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte,b'!'|b'#'..=b'\''|b'*'|b'+'|b'-'|b'.'|b'^'..=b'`'|b'|'|b'~')
}
/// Supported Host subset: ASCII DNS-style labels / IPv4 names, optional
/// decimal port. IPv6 literals, userinfo and percent-encoded names are rejected.
fn host(value: &[u8]) -> bool {
    // 0: empty label, 1: alphanumeric end, 2: hyphen end, 3: root dot.
    let mut state = 0;
    let mut bytes = value.iter().copied();
    while let Some(byte) = bytes.next() {
        match byte {
            b':' => {
                if !matches!(state, 1 | 3) || bytes.len() == 0 {
                    return false;
                }
                let mut port = 0u32;
                for digit in bytes {
                    if !digit.is_ascii_digit() {
                        return false;
                    }
                    port = port * 10 + (digit - b'0') as u32;
                    if port > 65535 {
                        return false;
                    }
                }
                return true;
            }
            b'.' if state == 1 => state = 3,
            b'-' if matches!(state, 1 | 2) => state = 2,
            _ if byte.is_ascii_alphanumeric() => state = 1,
            _ => return false,
        }
    }
    matches!(state, 1 | 3)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{format, vec::Vec};
    fn request(body: &[u8]) -> Vec<u8> {
        let mut r = format!(
            "POST /echo HTTP/1.1\r\nHost: 192.168.123.50\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        r.extend_from_slice(body);
        r
    }
    #[test]
    fn response_framing_has_exact_status_length_and_close_for_all_sizes() {
        for status in [
            Status::Ok,
            Status::BadRequest,
            Status::Timeout,
            Status::LengthRequired,
            Status::TooLarge,
            Status::UriTooLong,
            Status::ExpectationFailed,
            Status::HeadersTooLarge,
        ] {
            for size in 0..=64 {
                let mut out = [0xa5; IO_CAPACITY];
                let len = response_header(status, size, &mut out);
                let text = core::str::from_utf8(&out[..len]).unwrap();
                assert_eq!(
                    text.lines().next(),
                    Some(format!("HTTP/1.1 {} Response", status.code()).as_str())
                );
                let field = text
                    .lines()
                    .find_map(|s| s.strip_prefix("Content-Length: "))
                    .unwrap();
                assert_eq!(field.parse::<u8>(), Ok(size));
                assert!(text.ends_with("Connection: close\r\n\r\n"));
                assert!(out[len..].iter().all(|b| *b == 0xa5));
            }
        }
    }
    #[test]
    fn all_body_sizes_and_every_split_preserve_binary_bytes() {
        assert!(core::mem::size_of::<Request>() <= 80);
        for size in 0..=CAPACITY {
            let body: Vec<_> = (0..size).map(|i| (i * 37) as u8).collect();
            let wire = request(&body);
            for chunk in [1, 3, 32, 1000] {
                let mut p = Request::new();
                let mut status = None;
                for part in wire.chunks(chunk) {
                    status = p.feed(part);
                }
                assert_eq!(status, Some(Status::Ok));
                assert_eq!(p.body(), body);
            }
            if size == 64 {
                for split in 0..=wire.len() {
                    let mut p = Request::new();
                    p.feed(&wire[..split]);
                    assert_eq!(p.feed(&wire[split..]), Some(Status::Ok));
                    assert_eq!(p.body(), body);
                }
            }
        }
    }
    #[test]
    fn framing_ambiguities_and_unsupported_forms_never_echo() {
        for headers in [
            "Content-Length: 0\r\n",
            "Host: a\r\nHost: b\r\nContent-Length: 0\r\n",
            "Host: a\r\nContent-Length: 0\r\nContent-Length: 0\r\n",
            "Host: a\r\nContent-Length: 0\r\nContent-Length: 1\r\n",
            "Host: a\r\nContent-Length : 0\r\n",
            "Host: a\r\n Content-Length: 0\r\n",
            "Host: a\r\nContent-Length: +0\r\n",
            "Host: a\r\nContent-Length: -1\r\n",
            "Host: a\r\nContent-Length: 0,0\r\n",
            "Host: a\r\nContent-Length: \r\n",
            "Host: a\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n",
            "Host: a\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n",
            "Host: a\r\nContent-Length: 0\r\nX: \x0b\r\n",
            "Host: a\r\nContent-Length: 0\r\nX: \0\r\n",
        ] {
            let mut p = Request::new();
            let wire = format!("POST /echo HTTP/1.1\r\n{headers}\r\n");
            assert_eq!(
                p.feed(wire.as_bytes()),
                Some(Status::BadRequest),
                "{headers:?}"
            );
            assert!(p.body().is_empty());
        }
        for wire in [
            b"GET /echo HTTP/1.1\r\n".as_slice(),
            b"POST /other HTTP/1.1\r\n",
            b"POST /echo HTTP/1.0\r\n",
            b"POST /echo HTTP/1.1\n",
            b"POST /echo HTTP/1.1\rX",
        ] {
            assert_eq!(Request::new().feed(wire), Some(Status::BadRequest));
        }
    }
    #[test]
    fn lengths_headers_expect_and_host_bounds_have_explicit_errors() {
        for (headers, status) in [
            ("Host: a\r\n", Status::LengthRequired),
            ("Host: a\r\nContent-Length: 65\r\n", Status::TooLarge),
            (
                "Host: a\r\nContent-Length: 999999999999999999\r\n",
                Status::TooLarge,
            ),
            (
                "Host: a\r\nExpect: 100-continue\r\nContent-Length: 0\r\n",
                Status::ExpectationFailed,
            ),
        ] {
            let wire = format!("POST /echo HTTP/1.1\r\n{headers}\r\n");
            assert_eq!(Request::new().feed(wire.as_bytes()), Some(status));
        }
        for h in ["localhost", "192.168.123.50:80", "a-b.example.:65535"] {
            assert!(host(h.as_bytes()));
        }
        for h in [
            "",
            ":",
            "a:",
            "a:65536",
            "a:999999999999999999999999",
            "a:8:9",
            "a/b",
            "a b",
            "[::1]",
            "user@a",
            "-a",
            "a-",
            "a..b",
        ] {
            assert!(!host(h.as_bytes()), "{h}");
        }
        let mut p = Request::new();
        assert_eq!(p.feed(&[b'P'; 65]), Some(Status::UriTooLong));
        let mut wire = b"POST /echo HTTP/1.1\r\nX:".to_vec();
        wire.extend_from_slice(&[b'a'; 65]);
        assert_eq!(Request::new().feed(&wire), Some(Status::HeadersTooLarge));
        let wire = format!("POST /echo HTTP/1.1\r\n{}\r\n", "X: a\r\n".repeat(17));
        assert_eq!(
            Request::new().feed(wire.as_bytes()),
            Some(Status::HeadersTooLarge)
        );
        let line = format!("X:{}\r\n", "a".repeat(62));
        let wire = format!("POST /echo HTTP/1.1\r\n{}\r\n", line.repeat(16));
        assert_eq!(
            Request::new().feed(wire.as_bytes()),
            Some(Status::HeadersTooLarge)
        );
    }
    #[test]
    fn partial_input_timeout_eof_reset_and_trailing_octets_are_bounded() {
        let wire = request(&[0xff, 0, 13, 10]);
        for cut in [0, 1, 20, wire.len() - 1] {
            let mut p = Request::new();
            assert_eq!(p.feed(&wire[..cut]), None);
            assert!(p.body().is_empty());
            assert_eq!(p.timeout(), Status::Timeout);
            assert!(p.body().is_empty());
            p.reset();
            p.feed(&wire[..cut]);
            assert_eq!(p.eof(), Status::BadRequest);
            p.reset();
            assert_eq!(p.feed(&request(&[])), Some(Status::Ok));
            assert!(p.body().is_empty());
        }
        let mut p = Request::new();
        let mut extra = wire.clone();
        extra.extend_from_slice(b"GET /next HTTP/1.1\r\n");
        assert_eq!(p.feed(&extra), Some(Status::Ok));
        assert_eq!(p.body(), &[0xff, 0, 13, 10]);
        assert_eq!(p.feed(b"ignored"), Some(Status::Ok));
        assert_eq!(p.timeout(), Status::Ok);
        p.reset();
        let wire = b"POST /echo HTTP/1.1\r\nhOsT:\ta.example\t\r\ncOnTeNt-LeNgTh: 0001 \t\r\n\r\nz";
        assert_eq!(p.feed(wire), Some(Status::Ok));
        assert_eq!(p.body(), b"z");
    }
}
