use std::{fs, io, path::Path};
pub fn read(path: &Path) -> io::Result<Vec<Vec<u8>>> {
    let b = fs::read(path)?;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported/truncated Ethernet PCAP",
        )
    };
    if b.len() < 24
        || b[..4] != [0xd4, 0xc3, 0xb2, 0xa1]
        || u32::from_le_bytes(b[20..24].try_into().unwrap()) != 1
    {
        return Err(invalid());
    }
    let mut frames = Vec::new();
    let mut offset = 24;
    while offset < b.len() {
        if offset + 16 > b.len() {
            return Err(invalid());
        }
        let len = u32::from_le_bytes(b[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let original = u32::from_le_bytes(b[offset + 12..offset + 16].try_into().unwrap()) as usize;
        offset += 16;
        if len != original || len > 1514 || offset + len > b.len() {
            return Err(invalid());
        }
        frames.push(b[offset..offset + len].to_vec());
        offset += len;
    }
    Ok(frames)
}
pub fn write(path: &Path, frames: &[Vec<u8>]) -> io::Result<()> {
    let mut b = Vec::new();
    b.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes());
    b.extend_from_slice(&[2, 0, 4, 0]);
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&65535u32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    for (i, f) in frames.iter().enumerate() {
        b.extend_from_slice(&(i as u32).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..2 {
            b.extend_from_slice(&(f.len() as u32).to_le_bytes());
        }
        b.extend_from_slice(f);
    }
    fs::write(path, b)
}
