//! Where the generated test media are: the set `scripts/gen-engine-fixtures.sh` makes for this checkout's version of
//! itself, `target/engine-fixtures/<version>/`. The version is the script's POSIX `cksum` (its CRC and its length), as
//! the script names the set, so a checkout reads the set its own script makes while checkouts and branches share the
//! folder (a shared build server). One copy, included with `#[path]` by the engine's tests and examples and by the
//! app's tests.

use std::path::{Path, PathBuf};

const SCRIPT: &str = "scripts/gen-engine-fixtures.sh";

/// This checkout's fixture folder (missing until scripts/gen-engine-fixtures.sh has made it).
pub fn fixture_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join(SCRIPT).is_file())
        .expect("the repository holds scripts/gen-engine-fixtures.sh");
    let script = std::fs::read(root.join(SCRIPT)).expect("scripts/gen-engine-fixtures.sh reads");
    root.join("target/engine-fixtures")
        .join(format!("{}-{}", cksum(&script), script.len()))
}

/// POSIX `cksum`'s CRC: polynomial 0x04C11DB7, most significant bit first, over the bytes and then the length's
/// significant bytes (least first), inverted.
fn cksum(data: &[u8]) -> u32 {
    let step = |crc: u32, byte: u8| {
        (0..8).fold(crc ^ (u32::from(byte) << 24), |c, _| {
            if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C1_1DB7
            } else {
                c << 1
            }
        })
    };
    let mut crc = data.iter().fold(0, |crc, &byte| step(crc, byte));
    let mut len = data.len();
    while len > 0 {
        crc = step(crc, len as u8);
        len >>= 8;
    }
    !crc
}
