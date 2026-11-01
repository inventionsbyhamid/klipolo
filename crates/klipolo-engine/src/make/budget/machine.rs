//! This computer, as far as a cost is concerned: its GPU backend, adapter and OS
//! release. A cost is kept under it, so a new GPU or OS release is checked again. On macOS the GPU driver ships with
//! the OS release, so a driver update is a new key. On Windows and Linux the driver isn't in the key yet (the release
//! is the OS's, or Linux's kernel): a driver update there keeps every verdict until Windows and Linux support adds
//! `AdapterInfo::driver`. Same-name GPU variants (an "Apple M2" with 8 or 10 GPU cores) share a key.

use klipolo_platform::caps::OsInfo;
use serde::{Deserialize, Serialize};

/// `backend|adapter|os release`, e.g. "metal|Intel(R) Iris(TM) Plus Graphics|15.6.1".
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineKey(String);

impl MachineKey {
    pub fn of(info: &wgpu::AdapterInfo, os: &OsInfo) -> Self {
        Self::from_parts(&format!("{:?}", info.backend), &info.name, &os.version)
    }

    /// From its parts: the backend's name (any case), the adapter's name and the OS release.
    pub fn from_parts(backend: &str, adapter: &str, os_version: &str) -> Self {
        Self(format!("{}|{adapter}|{os_version}", backend.to_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// This computer's key as the render worker builds it: the adapter every Klipolo device
/// is made on (`choose_adapter`: the same instance and request as the worker's `RenderDevice`, which reads `get_info`
/// of it) and the OS release (`caps::current`), with no device made. None without an adapter. The app keys the verified
/// store with it, so a cost the worker measured here is found here.
pub async fn this_computer() -> Option<MachineKey> {
    use crate::vendor::cap::rendering::{choose_adapter, create_wgpu_instance};
    let instance = create_wgpu_instance().await;
    let (adapter, _) = choose_adapter(&instance).await.ok()?;
    Some(MachineKey::of(
        &adapter.get_info(),
        &klipolo_platform::caps::current().os,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use klipolo_platform::caps::OsFamily;

    #[test]
    fn the_machine_key_names_the_backend_the_adapter_and_the_os_release() {
        let info = wgpu::AdapterInfo {
            name: "Intel(R) Iris(TM) Plus Graphics".into(),
            vendor: 0x8086,
            device: 0,
            device_type: wgpu::DeviceType::IntegratedGpu,
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Metal,
        };
        let os = OsInfo {
            family: OsFamily::Macos,
            version: "15.6.1".into(),
            arch: "x86_64".into(),
            cpu_arch: "x86_64".into(),
        };
        assert_eq!(
            MachineKey::of(&info, &os).as_str(),
            "metal|Intel(R) Iris(TM) Plus Graphics|15.6.1"
        );
    }
}
