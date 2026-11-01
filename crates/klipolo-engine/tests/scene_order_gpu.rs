//! A group's effects in CSS's order on the GPU (lavapipe in CI): its filters before its mask, and its own clip in its
//! own, turned space. Goldens within 2/255.

mod support;
use support::*;

/// A white band blurred by 30 px, shown through a hidden rect it runs past: CSS blurs it, then masks it, so the
/// mask's edge stays sharp while the band's own edge inside it is soft. Masked first and blurred after, the white bled
/// about 10 px past the mask's edge.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_masked_group_blurs_before_its_mask() {
    let _gpu = gpu_turn().await;
    const MASKED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0,0,1,1],"fill":"#101010"},
      {"type":"rect","id":"m","box":[0.1,0.1,0.4,0.8],"fill":"#FFFFFF","opacity":0},
      {"type":"group","box":[0,0,1,1],"blur":30,"mask":{"node":"#m"},"children":[
        {"type":"rect","box":[0.05,0.05,0.9,0.45],"fill":"#FFFFFF"}]}]}"##;
    let (img, notes) = scene_frame_at(MASKED, 1.0, None).await;
    keep_golden("mask-after-blur", &img);
    assert!(notes.is_empty(), "{notes:?}");
    let luma = |x: u32, y: u32| luma_of(rgb_at(&img, x, y));
    // The mask's right edge is at x = 320; the band's lower edge at y = 180, inside the mask.
    let (inside, outside) = (luma(314, 100), luma(326, 100));
    assert!(inside > 200.0, "white inside the mask: {inside}");
    assert!(outside < 26.0, "nothing past the mask's edge: {outside}");
    let edge = luma(200, 180);
    assert!(
        edge > 60.0 && edge < 200.0,
        "the band's own edge blurs: {edge}"
    );
    scene_golden("mask-after-blur", &img);
}

/// A group tipped back by `rotateX`, clipped to the left three quarters of its own box: its clip's edge turns with
/// it, standing out farther on its near side than on its far one. Taken in the frame's pixels, the edge was upright.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_tilted_groups_own_clip_turns_with_it() {
    let _gpu = gpu_turn().await;
    const TIPPED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0,0,1,1],"fill":"#101418"},
      {"type":"group","box":[0.2,0.1,0.6,0.8],"rotateX":45,"perspective":1200,"clip":"inset(0 25% 0 0)","children":[
        {"type":"rect","box":[0,0,1,1],"fill":"#F2C94C"}]}]}"##;
    let (img, notes) = scene_frame_at(TIPPED, 1.0, None).await;
    keep_golden("tilt-clip", &img);
    assert!(notes.is_empty(), "{notes:?}");
    let lit = |x: u32, y: u32| luma_of(rgb_at(&img, x, y)) > 80.0;
    let rows: Vec<u32> = (0..360).filter(|&y| (0..640).any(|x| lit(x, y))).collect();
    let (top, bottom) = (*rows.first().unwrap() + 6, *rows.last().unwrap() - 6);
    let right = |y: u32| (0..640).rev().find(|&x| lit(x, y)).unwrap();
    let (far, near) = {
        let (a, b) = (right(top), right(bottom));
        (a.min(b), a.max(b))
    };
    assert!(
        near > far + 20,
        "the clip's edge turns: {near} px on the near side, {far} px on the far one"
    );
    scene_golden("tilt-clip", &img);
}
