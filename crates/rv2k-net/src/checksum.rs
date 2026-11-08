/// RFC Internet checksum, preserving an odd byte across arbitrary chunks.
#[derive(Default)]
pub struct Checksum {
    sum: u32,
    high: Option<u8>,
}
impl Checksum {
    pub const fn new() -> Self {
        Self { sum: 0, high: None }
    }
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if let Some(high) = self.high.take() {
                self.sum += u16::from_be_bytes([high, *byte]) as u32;
                self.sum = (self.sum & 0xffff) + (self.sum >> 16);
            } else {
                self.high = Some(*byte);
            }
        }
    }
    pub fn finish(&self) -> u16 {
        let mut sum = self.sum + self.high.map_or(0, |b| (b as u32) << 8);
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }
}
pub fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = Checksum::new();
    sum.update(bytes);
    sum.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn odd_chunk_boundaries_and_known_vector() {
        let data = [0, 1, 0xf2, 3, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&data), 0x220d);
        for len in 0..=data.len() {
            for chunk in 1..=9 {
                let mut c = Checksum::new();
                for part in data[..len].chunks(chunk) {
                    c.update(part);
                }
                assert_eq!(c.finish(), checksum(&data[..len]));
            }
        }
        let mut c = Checksum::new();
        for _ in 0..100000 {
            c.update(&[0xff; 3]);
        }
        assert_eq!(c.finish(), 0);
    }
}
