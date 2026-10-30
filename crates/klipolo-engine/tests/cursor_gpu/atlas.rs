//! The shapes on the GPU: a shape still loading draws the Arrow in preview while export preloads it, the atlas stays
//! within its size and lets the least recently drawn shapes go, and images over 256² or outside their take are refused.

use std::path::Path;
use std::time::{Duration, Instant};

use klipolo_engine::plan::CursorDraw;
use klipolo_engine::render::cursor::{ATLAS_BYTES, CursorPass, CursorShapeError, decode};

use crate::{device, layer, pass, preload_list, render, take_with};

#[test]
#[ignore = "needs a GPU"]
fn a_shape_still_loading_draws_the_arrow_in_preview_and_preload_makes_it_resident() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let blue = [0, 0, 255, 255];
    let (take, looks) = take_with(dir.path(), 1, &[(32, 32, blue), (300, 10, blue)]);
    let shape = layer(looks[0].clone(), (0.5, 0.5), 0.25);
    let arrow = layer(CursorDraw::Arrow, (0.5, 0.5), 0.25);
    let stand_in = render(&dev, &mut pass(&dev), &[arrow], false).0;
    // Preview: the first frame draws the Arrow look in its place and asks for it; it arrives off the render thread.
    let mut preview = pass(&dev);
    assert_eq!(
        render(&dev, &mut preview, std::slice::from_ref(&shape), false).0,
        stand_in
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let drawn = loop {
        let f = render(&dev, &mut preview, std::slice::from_ref(&shape), false);
        if preview.resident(&take, 0) {
            break f;
        }
        assert!(Instant::now() < deadline, "the shape never arrived");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        drawn.at(128 + 16, 72 + 16),
        blue,
        "its own shape once there"
    );
    // Export preloads before frame 1: resident at once, drawn from the first frame, the same pixels as the preview.
    let mut export = pass(&dev);
    let refused = export.preload(&dev.device, &dev.queue, &preload_list(&looks));
    let too_large = CursorShapeError::TooLarge {
        width: 300,
        height: 10,
    };
    assert_eq!(refused, Err(too_large), "the one it can't use is said");
    assert!(export.resident(&take, 0) && !export.resident(&take, 1));
    assert_eq!(render(&dev, &mut export, &[shape], true).0, drawn.0);
    // The refused one draws the Arrow look, in export and preview alike.
    let bad = layer(looks[1].clone(), (0.5, 0.5), 0.25);
    assert_eq!(render(&dev, &mut export, &[bad], true).0, stand_in);
}

fn colour(i: usize) -> [u8; 4] {
    [i as u8, (i * 7) as u8, 255 - i as u8, 255]
}

#[test]
#[ignore = "needs a GPU"]
fn the_atlas_never_passes_its_size_and_evicts_by_shape() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    // 200 shapes of 128 px from 4 takes; 64 fit at once.
    let mut all = Vec::new();
    for k in 0..4 {
        let shapes: Vec<_> = (0..50).map(|i| (128, 128, colour(k * 50 + i))).collect();
        let (take, looks) = take_with(dir.path(), k + 1, &shapes);
        all.extend(looks.into_iter().map(|l| (take.clone(), l)));
    }
    let mut p = pass(&dev);
    let draw = |p: &mut CursorPass, i: usize| {
        let f = render(&dev, p, &[layer(all[i].1.clone(), (0.1, 0.1), 0.5)], true);
        assert!(p.gpu_bytes() <= 6_000_000, "{} bytes", p.gpu_bytes());
        f
    };
    let resident = |p: &CursorPass, i: usize| p.resident(&all[i].0, (i % 50) as u16);
    for i in 0..64 {
        draw(&mut p, i);
    }
    assert!((0..64).all(|i| resident(&p, i)), "all fit");
    // Shape 0 drawn again: shape 1, of the same take, is now the least recently drawn and goes first, then shape 2.
    draw(&mut p, 0);
    draw(&mut p, 64);
    assert!(resident(&p, 0) && !resident(&p, 1) && resident(&p, 2));
    draw(&mut p, 65);
    assert!(!resident(&p, 2) && resident(&p, 3));
    for i in 66..200 {
        draw(&mut p, i);
    }
    assert!((136..200).all(|i| resident(&p, i)) && !resident(&p, 135) && !resident(&p, 0));
    // One let go long ago comes back into a reused cell whole: its own colour, nothing of the shape before it.
    let f = draw(&mut p, 1);
    assert_eq!(f.at(26 + 36, 14 + 36), colour(1));
}

#[test]
#[ignore = "needs a GPU"]
fn a_fresh_pass_preloads_what_fits_and_export_draws_every_real_shape() {
    // 20 shapes of 200² (cells of 256: 16 fit) preloaded into a fresh pass, before its first frame.
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let shapes: Vec<_> = (0..20).map(|i| (200, 200, colour(i))).collect();
    let (take, looks) = take_with(dir.path(), 1, &shapes);
    let mut export = pass(&dev);
    assert_eq!(
        export.preload(&dev.device, &dev.queue, &preload_list(&looks)),
        Ok(())
    );
    assert!(
        (0..20).all(|i| !export.failed(&take, i)),
        "none refused for want of room"
    );
    assert!(
        (0..16).all(|i| export.resident(&take, i)),
        "the first 16 in, in order"
    );
    // The rest load as the export draws them: every frame its own shape, never the Arrow.
    for (i, look) in looks.iter().enumerate() {
        let f = render(
            &dev,
            &mut export,
            &[layer(look.clone(), (0.1, 0.1), 0.5)],
            true,
        );
        assert_eq!(f.at(26 + 36, 14 + 36), colour(i), "shape {i}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn forget_lets_the_atlas_go_and_a_later_frame_loads_its_shape_again() {
    // The project closes or the window hides.
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let blue = [0, 0, 255, 255];
    let (take, looks) = take_with(dir.path(), 1, &[(32, 32, blue), (32, 32, [0, 255, 0, 255])]);
    let mut p = pass(&dev);
    let bare = p.gpu_bytes();
    p.preload(&dev.device, &dev.queue, &preload_list(&looks[..1]))
        .unwrap();
    assert_eq!(p.gpu_bytes(), bare + ATLAS_BYTES);
    p.forget(&dev.device);
    assert_eq!(
        p.gpu_bytes(),
        bare,
        "only the ring and the placeholder left"
    );
    assert!(!p.resident(&take, 0));
    // A preview frame then draws the stand-in, asks for the shape again and draws it once it is back.
    let shape = layer(looks[0].clone(), (0.5, 0.5), 0.25);
    let arrow = layer(CursorDraw::Arrow, (0.5, 0.5), 0.25);
    let stand_in = render(&dev, &mut pass(&dev), &[arrow], false).0;
    assert_eq!(
        render(&dev, &mut p, std::slice::from_ref(&shape), false).0,
        stand_in
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !p.resident(&take, 0) {
        assert!(Instant::now() < deadline, "the shape never came back");
        std::thread::sleep(Duration::from_millis(20));
        render(&dev, &mut p, &[], false);
    }
    assert_eq!(
        render(&dev, &mut p, &[shape], false).at(128 + 16, 72 + 16),
        blue
    );
    // A decode asked for before a forget, and done after it, is dropped: nothing goes in, nothing is made.
    render(
        &dev,
        &mut p,
        &[layer(looks[1].clone(), (0.5, 0.5), 0.25)],
        false,
    );
    p.forget(&dev.device);
    std::thread::sleep(Duration::from_millis(300));
    render(&dev, &mut p, &[], false);
    assert!(!p.resident(&take, 1) && p.gpu_bytes() == bare);
}

#[test]
fn a_png_over_256_or_outside_its_take_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let red = [255, 0, 0, 255];
    let (take, _) = take_with(
        dir.path(),
        1,
        &[(300, 10, red), (8, 8, [200, 100, 50, 128])],
    );
    let (_, _) = take_with(dir.path(), 2, &[(8, 8, red)]);
    let take = Path::new(&*take);
    let cursors = dir.path().join("take1").join("cursors");
    let too_large = CursorShapeError::TooLarge {
        width: 300,
        height: 10,
    };
    assert_eq!(decode(take, &cursors.join("0.png")), Err(too_large));
    // A usable one, premultiplied.
    let ok = decode(take, &cursors.join("1.png")).unwrap();
    assert_eq!(
        (ok.width, ok.height, &ok.rgba[..4]),
        (8, 8, &[100, 50, 25, 128][..])
    );
    // Another take's shape, one beside the take's `cursors/`, and a link out of it.
    let other = dir.path().join("take2").join("cursors").join("0.png");
    assert_eq!(decode(take, &other), Err(CursorShapeError::OutsideTake));
    let beside = dir.path().join("take1").join("0.png");
    std::fs::copy(&other, &beside).unwrap();
    assert_eq!(decode(take, &beside), Err(CursorShapeError::OutsideTake));
    #[cfg(unix)]
    {
        let link = cursors.join("9.png");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert_eq!(decode(take, &link), Err(CursorShapeError::OutsideTake));
    }
    // Not a PNG at all, or not there.
    std::fs::write(cursors.join("8.png"), b"not a png").unwrap();
    assert!(matches!(
        decode(take, &cursors.join("8.png")),
        Err(CursorShapeError::Unreadable(_))
    ));
    assert!(matches!(
        decode(take, &cursors.join("7.png")),
        Err(CursorShapeError::Unreadable(_))
    ));
}
