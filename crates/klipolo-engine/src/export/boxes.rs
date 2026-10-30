//! Reading an MP4's top-level boxes (to check its index comes first and holds no edit list) without a demuxer.

use std::path::Path;

/// The file's top-level boxes: type, offset and size.
pub(super) fn top_level_boxes(path: &Path) -> std::io::Result<Vec<([u8; 4], u64, u64)>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let (mut at, mut boxes) = (0u64, Vec::new());
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8])?;
        let mut size = u64::from(u32::from_be_bytes(head[..4].try_into().expect("4 bytes")));
        let kind: [u8; 4] = head[4..8].try_into().expect("4 bytes");
        if size == 1 {
            file.read_exact(&mut head[8..16])?;
            size = u64::from_be_bytes(head[8..16].try_into().expect("8 bytes"));
        } else if size == 0 {
            size = len - at;
        }
        if size < 8 {
            break;
        }
        boxes.push((kind, at, size));
        at += size;
    }
    Ok(boxes)
}

pub(super) fn read_range(path: &Path, offset: u64, size: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; size.min(256 << 20) as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}
