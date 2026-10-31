//! This Mac's id with Klipolo's relay: a random UUID made on the first launch and kept in app support
//! (`Klipolo/device.json`). It is not a secret (the relay token is); it only names the device when it registers.

use std::io;
use std::path::Path;

use klipolo_platform::durable::{Durability, write_atomic};
use serde::{Deserialize, Serialize};

/// The file in the app support folder that keeps the id.
pub const DEVICE_FILE: &str = "device.json";

#[derive(Serialize, Deserialize)]
struct DeviceFile {
    device_id: String,
}

/// This device's id, read from `dir/device.json`, or made and saved there the first time (`dir` is created). A file
/// that can't be read as an id is replaced by a new one: the relay then sees a new device, which starts without a
/// code, as a first launch does.
pub fn device_id(dir: &Path) -> io::Result<String> {
    let path = dir.join(DEVICE_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<DeviceFile>(&bytes) {
            Ok(file) if uuid::Uuid::parse_str(&file.device_id).is_ok() => return Ok(file.device_id),
            _ => tracing::warn!("the device id file couldn't be read, so this Mac gets a new id"),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(&DeviceFile {
        device_id: id.clone(),
    })
    .map_err(io::Error::other)?;
    write_atomic(&path, &bytes, Durability::Full)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_file_gets_a_new_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(DEVICE_FILE),
            b"{\"device_id\": \"not a uuid\"}",
        )
        .unwrap();
        let id = device_id(dir.path()).unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(device_id(dir.path()).unwrap(), id);
    }
}
