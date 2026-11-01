//! ISO BMFF boxes: writing them in place into one buffer (no allocation per box), and reading them from a slice.

/// Appends a box: its size is patched in after `body` wrote the contents.
pub(crate) fn bx(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    body(out);
    let size = u32::try_from(out.len() - at).expect("an MP4 box under 4 GB");
    out[at..at + 4].copy_from_slice(&size.to_be_bytes());
}

/// Appends a full box: a box whose body starts with a version byte and 24 bits of flags.
pub(crate) fn full(
    out: &mut Vec<u8>,
    kind: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    bx(out, kind, |o| {
        be32(o, (u32::from(version) << 24) | (flags & 0x00ff_ffff));
        body(o);
    });
}

pub(crate) fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn be64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Writes a u32 count where `at` reserved it (tables write their entries before they know how many).
pub(crate) fn patch32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// A big-endian reader over a slice; every read is None past the end, so malformed boxes fail softly.
pub(crate) struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    pub(crate) fn new(b: &'a [u8]) -> Self {
        Self { b, at: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A full box's version and flags.
    pub(crate) fn version_flags(&mut self) -> Option<(u8, u32)> {
        let v = self.u32()?;
        Some(((v >> 24) as u8, v & 0x00ff_ffff))
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.b[self.at.min(self.b.len())..]
    }
}

/// The boxes in `b`, in order, as (type, body); stops at the first malformed one.
pub(crate) fn children(b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let head = b.get(at..at + 8)?;
        let size32 = u32::from_be_bytes(head[..4].try_into().ok()?);
        let kind: [u8; 4] = head[4..8].try_into().ok()?;
        let (header, size) = match size32 {
            0 => (8, b.len() - at),
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(b.get(at + 8..at + 16)?.try_into().ok()?))
                    .ok()?,
            ),
            n => (8, n as usize),
        };
        if size < header {
            return None;
        }
        let body = b.get(at + header..at.checked_add(size)?)?;
        at += size;
        Some((kind, body))
    })
}

/// The body of `b`'s first child of type `kind`.
pub(crate) fn child<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(b).find(|(k, _)| k == kind).map(|(_, body)| body)
}

/// The body at the end of a child path, e.g. `["mdia", "minf", "stbl"]`.
pub(crate) fn path<'a>(b: &'a [u8], kinds: &[&[u8; 4]]) -> Option<&'a [u8]> {
    kinds.iter().try_fold(b, |b, k| child(b, k))
}
