// Shape mask: keeps the layer inside a shape and clears everything outside it. `shape` 0 circle, 1 rounded square
// (corners `cornerRadius` of its side), 2 square: each as big as the layer's shorter side, so a 16:9 facecam becomes
// a true circle. `center` moves it across the room it has inside the layer: on each axis 0 is as far left (up) as it
// fits and 1 as far right (down), so it slides only along the longer side. 3 frame: the whole layer with corners
// `cornerRadius` of its shorter side (it has no room to move). The edge fades over `feather` pixels inside the
// shape, on top of a one-pixel analytic anti-alias; `borderWidth` pixels inside the edge take `borderColor` (laid
// over the layer, so a translucent colour tints it). One sample, no loops, no branches. Measured from the layer's own box
// (`kl.content`), so a padded filter's margin never moves or grows it.

// Signed distance in pixels from `q` (from the shape's centre) to a box of half-size `h` whose corners have radius
// `rad`, negative inside. A square box with `rad` half its side is a circle, exactly.
fn mask_box(q: vec2<f32>, h: vec2<f32>, rad: f32) -> f32 {
    let e = abs(q) - h + vec2<f32>(rad);
    return length(max(e, vec2<f32>(0.0))) + min(max(e.x, e.y), 0.0) - rad;
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = kl_sample_a(uv);
    let size = kl_content_size();
    let side = min(size.x, size.y);
    let frame = params.shape == 3u;
    // Half-size and centre in pixels. `center` spans the shape's travel on each axis (from `h` to `size - h`), so the
    // whole slider moves it; an axis with no travel (the shorter side, or a frame's either side) stays centred.
    let h = select(vec2<f32>(side * 0.5), size * 0.5, frame);
    let centre = h + clamp(params.center, vec2<f32>(0.0), vec2<f32>(1.0)) * (size - 2.0 * h);
    // Corner radius as a share of the shorter side: a circle is a square rounded by half its side.
    var corner = clamp(params.cornerRadius, 0.0, 0.5);
    corner = select(corner, 0.5, params.shape == 0u);
    corner = select(corner, 0.0, params.shape == 2u);
    let d = mask_box(kl_content_uv(uv) * size - centre, h, corner * side);
    // Coverage: whole from `feather` inside the edge, none from half a pixel outside it. With no feather the edge
    // still ramps over the one pixel it crosses.
    let feather = max(params.feather, 0.0) * kl.px;
    let inside = 1.0 - smoothstep(-feather - 0.5, 0.5, d);
    // The border: the ring `borderWidth` deep inside the edge, its inner edge anti-aliased too. Under a pixel wide it
    // fades in with its width rather than snapping on.
    let width = max(params.borderWidth, 0.0) * kl.px;
    let ring = smoothstep(-width - 0.5, -width + 0.5, d) * min(width, 1.0);
    let border = kl_premultiply(params.borderColor);
    let lined = mix(c, border + c * (1.0 - border.a), ring);
    // Premultiplied, so every channel scales with coverage.
    return lined * inside;
}
