//! The glyph atlas stays bounded (run by hand: `cargo test -p klipolo-engine --test text_atlas -- --ignored`). Its own
//! test binary: it reads the process's Metal allocation counter, which other GPU tests running in parallel would move.
//! macOS only: other GPU APIs report their allocations differently.
#![cfg(target_os = "macos")]

mod support;
use support::bx;

/// Bytes the Metal device has allocated (textures and buffers), after the GPU has let go of dropped ones.
fn gpu_bytes(dev: &klipolo_engine::render::device::RenderDevice) -> u64 {
    let _ = dev.device.poll(wgpu::PollType::Wait);
    // SAFETY: only reads the device's allocation counter.
    unsafe {
        dev.device.as_hal::<wgpu_hal::api::Metal, _, _>(|d| {
            d.map_or(0, |d| d.raw_device().lock().current_allocated_size())
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn glyph_atlas_stays_bounded_while_the_size_slider_moves() {
    use klipolo_engine::plan::TextLayer;
    use klipolo_engine::render::text::TextDrawer;
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let layer = |px: f64| TextLayer {
        id: "t".into(),
        owner: klipolo_engine::plan::TextOwner::Layer,
        text: "The quick brown fox jumps over 0123456789 lazy dogs".into(),
        style: klipolo_model::timeline::TextStyle::default().into(),
        font_px: px,
        dest: bx(0.0, 0.0, 1.0, 0.6),
        rotation: 0.0,
        opacity: 1.0,
        scale: 1.0,
        reveal: 1.0,
        effects: Vec::new(),
        pad_px: 0.0,
        blend: klipolo_model::motion::Blend::Normal,
        reach: 1.0,
        language: "en".into(),
    };
    // A size slider dragged from 100 to 400 px and back, a few pixels off: every size rasterizes every glyph again.
    // The first sweep grows the atlas to what the largest size needs; the second must fit in it.
    let mut sweep = |offset: u32| {
        for px in (100..=400).step_by(10) {
            let px = f64::from(px + offset);
            drawer.prepare(
                &dev.device,
                &dev.queue,
                &layer(px),
                (1920, 1080),
                (960, 540),
                &mut Vec::new(),
            );
            drawer.end_frame();
        }
    };
    sweep(0);
    let baseline = gpu_bytes(&dev);
    sweep(5);
    let grown = gpu_bytes(&dev).saturating_sub(baseline);
    // Untrimmed, every glyph of the first sweep stays in use and the mask atlas doubles again (+51 MiB measured).
    assert!(grown < 4 << 20, "GPU memory grew by {} KiB", grown >> 10);
}

/// The glyph atlas doesn't stay grown after a big title: 36 letters at 600 px (a 4K
/// title) grow it to about 4096² (16 MiB, more for its re-uploads), and once that title is drawn it is made anew, so a
/// small title after it leaves Metal's allocations about where they were before the big one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_glyph_atlas_shrinks_back_after_a_big_title() {
    use klipolo_engine::plan::TextLayer;
    use klipolo_engine::render::text::TextDrawer;
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let layer = |text: &str, px: f64| TextLayer {
        id: "t".into(),
        owner: klipolo_engine::plan::TextOwner::Layer,
        text: text.into(),
        style: klipolo_model::timeline::TextStyle::default().into(),
        font_px: px,
        dest: bx(0.0, 0.0, 1.0, 1.0),
        rotation: 0.0,
        opacity: 1.0,
        scale: 1.0,
        reveal: 1.0,
        effects: Vec::new(),
        pad_px: 0.0,
        blend: klipolo_model::motion::Blend::Normal,
        reach: 1.0,
        language: "en".into(),
    };
    let mut draw = |text: &str, px: f64| {
        drawer.prepare(
            &dev.device,
            &dev.queue,
            &layer(text, px),
            (1920, 1080),
            (3840, 2160),
            &mut Vec::new(),
        );
        drawer.end_frame();
    };
    draw("Hi", 72.0);
    let baseline = gpu_bytes(&dev);
    draw("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789", 300.0);
    let grown = gpu_bytes(&dev).saturating_sub(baseline);
    draw("Hi", 72.0);
    let after = gpu_bytes(&dev).saturating_sub(baseline);
    println!(
        "GPU memory over the small title's: {} MiB with the big title, {} MiB after it",
        grown >> 20,
        after >> 20
    );
    assert!(
        after < 4 << 20,
        "the atlas made anew: {} KiB more",
        after >> 10
    );
}
