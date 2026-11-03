//! The GPU seam: the graphics API per OS, what kind of GPU the adapter is, and how export learns that
//! submitted work finished. The adapter is still requested with `PowerPreference::LowPower` everywhere (the integrated
//! GPU, where VideoToolbox's IOSurfaces live on a Mac); the per-job adapter policy (preview on the display's GPU,
//! export on the encoder's) is still to come, with Windows.

/// Apple's PCI vendor id.
pub const APPLE_VENDOR: u32 = 0x106B;

/// The instance every device is made from:
/// - macOS: Metal;
/// - Windows: DX12 (with FXC for now; a bundled DXC is still to come);
/// - Linux: Vulkan, with GL as the last resort.
///
/// Off macOS, `WGPU_BACKEND` narrows the choice (the Linux CI picks lavapipe with `WGPU_BACKEND=vulkan`).
pub fn instance_descriptor() -> wgpu::InstanceDescriptor {
    #[cfg(target_os = "macos")]
    let backends = wgpu::Backends::METAL;
    #[cfg(windows)]
    let backends = wgpu::Backends::DX12;
    #[cfg(not(any(target_os = "macos", windows)))]
    let backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
    #[cfg(not(target_os = "macos"))]
    let backends = wgpu::Backends::from_env().unwrap_or(backends);
    wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    }
}

/// Calls `done` (on a driver thread) once the GPU has finished the work submitted on `queue` so far, without blocking
/// anyone: wgpu's blocking poll holds the device's fence lock while it waits, which would stall the next submit.
/// - macOS: an empty command buffer committed on wgpu's Metal queue, whose completed handler calls it (Metal runs a
///   queue's command buffers in order);
/// - elsewhere: false, nothing is called (later a fence or timeline semaphore), and the caller treats the work as done.
pub fn on_submitted_done(queue: &wgpu::Queue, done: impl Fn() + 'static) -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the hal queue is wgpu's; its mutex is held while the command buffer is made, as wgpu-hal's submit
        // does, and committing after wgpu's own submit orders it after that work.
        unsafe {
            queue.as_hal::<wgpu_hal::api::Metal, _, _>(|queue| {
                let queue = queue?.as_raw().lock();
                let handler =
                    block::ConcreteBlock::new(move |_: &metal::CommandBufferRef| done()).copy();
                objc2::rc::autoreleasepool(|_| {
                    let buffer = queue.new_command_buffer_with_unretained_references();
                    buffer.set_label("Klipolo export frame done");
                    buffer.add_completed_handler(&handler);
                    buffer.commit();
                });
                Some(())
            })
        }
        .is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (queue, done);
        false
    }
}

/// What kind of GPU an adapter is; the preview's pixel budgets follow it, and so do editing copies (the engine reports
/// it in `EngineInfo::gpu_class`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GpuClass {
    /// Apple silicon.
    Apple,
    Integrated,
    Discrete,
    /// A CPU rasteriser (lavapipe, WARP) or a virtual GPU.
    Software,
}

impl GpuClass {
    /// From the adapter's info: Apple by vendor id, or (Metal reports no vendor id) by an "Apple" adapter name on
    /// Metal, as the budget did before; otherwise by device type (unknown types count as integrated: the smaller
    /// budget).
    pub fn of(info: &wgpu::AdapterInfo) -> Self {
        if info.vendor == APPLE_VENDOR
            || (info.backend == wgpu::Backend::Metal && info.name.contains("Apple"))
        {
            return Self::Apple;
        }
        match info.device_type {
            wgpu::DeviceType::DiscreteGpu => Self::Discrete,
            wgpu::DeviceType::Cpu | wgpu::DeviceType::VirtualGpu => Self::Software,
            wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::Other => Self::Integrated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{Backend, DeviceType};

    fn info(
        name: &str,
        vendor: u32,
        device_type: DeviceType,
        backend: Backend,
    ) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: name.into(),
            vendor,
            device: 0,
            device_type,
            driver: String::new(),
            driver_info: String::new(),
            backend,
        }
    }

    #[test]
    fn apple_comes_from_the_vendor_id_or_the_metal_name_and_the_rest_from_the_device_type() {
        let apple = info(
            "Apple M2",
            APPLE_VENDOR,
            DeviceType::IntegratedGpu,
            Backend::Vulkan,
        );
        assert_eq!(GpuClass::of(&apple), GpuClass::Apple);
        let metal = info("Apple M1", 0, DeviceType::IntegratedGpu, Backend::Metal);
        assert_eq!(GpuClass::of(&metal), GpuClass::Apple);
        let intel = info(
            "Intel(R) Iris(TM) Plus Graphics",
            0,
            DeviceType::IntegratedGpu,
            Backend::Metal,
        );
        assert_eq!(GpuClass::of(&intel), GpuClass::Integrated);
        let nvidia = info(
            "NVIDIA GeForce RTX 4070",
            0x10DE,
            DeviceType::DiscreteGpu,
            Backend::Dx12,
        );
        assert_eq!(GpuClass::of(&nvidia), GpuClass::Discrete);
        let lavapipe = info(
            "llvmpipe (LLVM 19.1.1, 256 bits)",
            0x10005,
            DeviceType::Cpu,
            Backend::Vulkan,
        );
        assert_eq!(GpuClass::of(&lavapipe), GpuClass::Software);
    }
}
