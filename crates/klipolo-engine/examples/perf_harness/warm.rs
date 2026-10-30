//! `cargo run -p klipolo-engine --example perf_harness -- warm [--home <ms> | --phases]`: the GPU side of the first
//! project open, with and without the warm-up at Home. Run each in a fresh process. For a cold shader
//! cache (the first launch after an install or update), run the binary as a new app: a copy inside a `.app` whose
//! bundle id Metal has no cache for.
//! - No flag: the engine's first start makes its backend, starts the native presenter and draws the first frames.
//! - `--home <ms>`: Home starts the warm-up and shows for `<ms>` before the click that opens a project; the engine's
//!   start takes the warmed backend, waiting for the warm-up if it is still running.
//! - `--phases [--utility]`: what each part of the warm-up costs.
//! - `--parts [--prime]`: what each pipeline group costs, in the order a first frame needs them (`--prime`: after
//!   another device in the process compiled them all).
//! - `--core [--parallel]`: the warm-up's first-frame set, one after another or in parallel, then the rest.
//!
//! Reports each step's ms from the click, and the footprint the warm-up holds at Home.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::render::mode::RenderMode;
use klipolo_engine::render::warm::Warmup;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::{ItemContent, TextStyle};

use super::effects::{base, effect, full_frame, with_adjustment};
use super::measure::{mb, me};
use super::project::secs;

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// `--phases [--utility]`: what each part of the warm-up costs, on this thread (at utility QoS with `--utility`).
fn phases(utility: bool) {
    use klipolo_engine::render::compositor::Compositor;
    use klipolo_engine::render::device::RenderDevice;
    use klipolo_engine::render::effects::EffectStage;
    if utility {
        klipolo_engine::qos::set_current(klipolo_engine::qos::Qos::Utility);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let started = Instant::now();
    let dev = runtime.block_on(RenderDevice::new()).expect("GPU");
    let device_ms = ms(started);
    let started = Instant::now();
    EffectStage::new(&dev, RenderMode::Contained).warm();
    let effects_ms = ms(started);
    let started = Instant::now();
    let mut compositor = Compositor::new(
        dev.clone(),
        Arc::new(tokio::sync::Notify::new()),
        RenderMode::Interactive,
    );
    let rest_ms = ms(started);
    let started = Instant::now();
    compositor.warm_all();
    let all_ms = ms(started);
    println!(
        "phases ({}): device {device_ms:.0} ms, effect pipelines {effects_ms:.0} ms, the compositor's own \
         {rest_ms:.0} ms, warm_all {all_ms:.0} ms",
        if utility {
            "utility QoS"
        } else {
            "default QoS"
        }
    );
}

/// Each part of the preview's GPU set-up on its own, in the order a first frame needs them, on `dev`: (name, ms).
fn parts(dev: &Arc<klipolo_engine::render::device::RenderDevice>) -> Vec<(&'static str, f64)> {
    use klipolo_engine::render::effects::EffectStage;
    use klipolo_engine::render::lut::LutTextures;
    use klipolo_engine::render::nv12::PlaneImporter;
    use klipolo_engine::render::nv12_scale::Nv12Scaler;
    use klipolo_engine::render::text::TextDrawer;
    use klipolo_engine::vendor::cap::rendering::composite_frame::CompositeVideoFramePipeline;
    use klipolo_engine::vendor::cap::rendering::frame_pipeline::RgbaToNv12Converter;
    use klipolo_engine::vendor::cap::rendering::layers::background::BackgroundLayer;
    use klipolo_engine::vendor::cap::rendering::layers::image::ImageLayer;
    use klipolo_engine::vendor::cap::rendering::yuv_converter::YuvConverterPipelines;
    let (device, queue) = (&dev.device, &dev.queue);
    let redraw = Arc::new(tokio::sync::Notify::new());
    let mut out = Vec::new();
    let mut time = |name, f: &mut dyn FnMut()| {
        let started = Instant::now();
        f();
        out.push((name, ms(started)));
    };
    let mut composite = None;
    time("composite NV12", &mut || {
        composite = Some(CompositeVideoFramePipeline::new(device))
    });
    let composite = composite.unwrap();
    time("composite RGBA", &mut || {
        let _ = composite.rgba_pipeline(device);
    });
    time("background", &mut || drop(BackgroundLayer::new(device)));
    time("LUT textures", &mut || {
        drop(LutTextures::new(device, queue, &composite, redraw.clone()))
    });
    time("plane importer", &mut || {
        let _ = PlaneImporter::new(device);
    });
    time("image quad", &mut || {
        drop(ImageLayer::new(device, redraw.clone()))
    });
    time("text", &mut || {
        TextDrawer::new(device, queue, false).warm(device)
    });
    time("YUV converters", &mut || {
        drop(YuvConverterPipelines::new(device))
    });
    time("NV12 scaler", &mut || drop(Nv12Scaler::new(device)));
    time("NV12 converter", &mut || {
        drop(RgbaToNv12Converter::new(device))
    });
    time("effects", &mut || {
        EffectStage::new(dev, RenderMode::Contained).warm()
    });
    out
}

/// `--parts [--prime]`: what each part of the GPU set-up costs on one device; with `--prime`, after a second device
/// in this process made them all first (does Metal's cache serve another device of the same GPU?).
fn parts_report(prime: bool) {
    use klipolo_engine::render::device::RenderDevice;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let started = Instant::now();
    let dev = runtime.block_on(RenderDevice::new()).expect("GPU");
    println!("device {:.0} ms", ms(started));
    if prime {
        let other = runtime.block_on(RenderDevice::new()).expect("GPU");
        let started = Instant::now();
        let primed = parts(&other);
        drop(other);
        println!(
            "primed on another device in {:.0} ms: {}",
            ms(started),
            line(&primed)
        );
    }
    let started = Instant::now();
    let done = parts(&dev);
    println!("parts in {:.0} ms: {}", ms(started), line(&done));
}

/// `--core [--parallel]`: the first-frame set the warm-up hands over first (the NV12 video composite, background, image
/// quad, text and the native presenter's copy), one after another or each on a thread of its own, then the rest.
fn core_report(parallel: bool) {
    use klipolo_engine::render::device::RenderDevice;
    use klipolo_engine::render::effects::EffectStage;
    use klipolo_engine::render::text::TextDrawer;
    use klipolo_engine::vendor::cap::rendering::composite_frame::CompositeVideoFramePipeline;
    use klipolo_engine::vendor::cap::rendering::layers::background::BackgroundLayer;
    use klipolo_engine::vendor::cap::rendering::layers::image::ImageLayer;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let started = Instant::now();
    let dev = runtime.block_on(RenderDevice::new()).expect("GPU");
    let device_ms = ms(started);
    let (device, queue) = (&dev.device, &dev.queue);
    let redraw = Arc::new(tokio::sync::Notify::new());
    let timed = |f: &dyn Fn()| {
        let started = Instant::now();
        f();
        ms(started)
    };
    let parts: [(&str, &(dyn Fn() + Sync)); 5] = [
        ("composite NV12", &|| {
            drop(CompositeVideoFramePipeline::new(device))
        }),
        ("background", &|| drop(BackgroundLayer::new(device))),
        ("image quad", &|| {
            drop(ImageLayer::new(device, redraw.clone()))
        }),
        ("text", &|| {
            TextDrawer::new(device, queue, false).warm(device)
        }),
        ("native copy", &|| {
            klipolo_engine::present::native::warm(device)
        }),
    ];
    let started = Instant::now();
    let each: Vec<f64> = if parallel {
        std::thread::scope(|s| {
            let threads: Vec<_> = parts.iter().map(|(_, f)| s.spawn(|| timed(*f))).collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        })
    } else {
        parts.iter().map(|(_, f)| timed(*f)).collect()
    };
    let core_ms = ms(started);
    let started = Instant::now();
    let composite = CompositeVideoFramePipeline::new(device);
    let rest = [
        (
            "composite RGBA",
            timed(&|| {
                let _ = composite.rgba_pipeline(device);
            }),
        ),
        (
            "effects",
            timed(&|| EffectStage::new(&dev, RenderMode::Contained).warm()),
        ),
    ];
    println!(
        "core ({}): device {device_ms:.0} ms, first-frame set {core_ms:.0} ms ({}); then {:.0} ms ({})",
        if parallel {
            "parallel"
        } else {
            "one after another"
        },
        parts
            .iter()
            .zip(&each)
            .map(|((name, _), ms)| format!("{name} {ms:.0}"))
            .collect::<Vec<_>>()
            .join(", "),
        ms(started),
        line(&rest),
    );
}

fn line(parts: &[(&str, f64)]) -> String {
    parts
        .iter()
        .map(|(name, ms)| format!("{name} {ms:.0}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn run(args: &[String]) {
    let mode = args.first().map(String::as_str).unwrap_or("");
    if mode == "--core" {
        let parallel = args.iter().any(|a| a == "--parallel");
        std::thread::spawn(move || core_report(parallel))
            .join()
            .unwrap();
        return;
    }
    if mode == "--parts" {
        let prime = args.iter().any(|a| a == "--prime");
        std::thread::spawn(move || parts_report(prime))
            .join()
            .unwrap();
        return;
    }
    if mode == "--phases" {
        let utility = args.iter().any(|a| a == "--utility");
        std::thread::spawn(move || phases(utility)).join().unwrap();
        return;
    }
    // `--home <ms>`: Home shows for that long, warming, before the click that opens a project.
    let home_ms = (mode == "--home").then(|| {
        args.get(1)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10_000)
    });
    let at_start = me().footprint;
    let warmup = home_ms.map(|_| Warmup::start(RenderMode::Interactive));
    if let Some(home_ms) = home_ms {
        tokio::time::sleep(Duration::from_millis(home_ms)).await;
        println!(
            "Home shown for {home_ms} ms, warming: footprint {:+.1} MB",
            mb(me().footprint) - mb(at_start)
        );
    }

    // The click: the engine's first start takes the warmed backend (waiting for the warm-up if it is still running)
    // or makes its own, then starts the native presenter and draws the first still and a playback frame.
    let open = Instant::now();
    let started = Instant::now();
    let mut backend = match warmup {
        Some(warmup) => warmup.finish().await.expect("a warmed backend"),
        None => GpuBackend::new(RenderMode::Interactive).await.expect("GPU"),
    };
    let new_ms = ms(started);
    let started = Instant::now();
    #[cfg(target_os = "macos")]
    let presenter = {
        use klipolo_engine::platform::NativeSurface;
        use klipolo_engine::present::macos::NativeLayer;
        let sink: klipolo_engine::present::native::ShownSink =
            Arc::new(|_: &klipolo_engine::frames::trailer::FrameMeta, _: f64, _: u32| {});
        backend
            .attach_native(NativeSurface::Metal(NativeLayer::detached()), sink)
            .expect("native presenter")
    };
    let attach_ms = ms(started);
    let mut project = with_adjustment(
        base(),
        full_frame(),
        vec![effect(
            "px",
            klipolo_model::contrib::builtin::EFFECT_PIXELATE,
        )],
    );
    project.timeline.main[0].effects =
        vec![effect("blur", klipolo_model::contrib::builtin::EFFECT_BLUR)];
    let mut title = project.timeline.tracks[0].items[0].clone();
    title.id = "title".into();
    title.effects.clear();
    title.content = ItemContent::Text {
        text: "Warm".into(),
        style: TextStyle::default(),
    };
    project.timeline.tracks[0].items.push(title);
    let plan = frame_plan(
        &PlanContext::new(Arc::new(project)),
        secs(1.0),
        FormatKind::Landscape,
    );
    let mut frame = async |output, width, height| {
        let started = Instant::now();
        let req = RenderRequest {
            plan: plan.clone(),
            width,
            height,
            output,
        };
        drop(backend.render(req).await.expect("render").finish().await);
        ms(started)
    };
    let still_ms = frame(Output::Rgba, 1280, 720).await;
    let nv12_ms = frame(Output::Nv12, 960, 540).await;
    println!(
        "first open ({}): {:.0} ms = backend {new_ms:.0} + native presenter {attach_ms:.0} + first still {still_ms:.0} \
         + first NV12 frame {nv12_ms:.0}; footprint now {:+.1} MB",
        home_ms.map_or("no warm-up".into(), |ms| format!("after {ms} ms at Home")),
        ms(open),
        mb(me().footprint) - mb(at_start)
    );
    let again = frame(Output::Rgba, 1280, 720).await;
    println!("  the same still again: {again:.0} ms");
    #[cfg(target_os = "macos")]
    drop(presenter);
}
