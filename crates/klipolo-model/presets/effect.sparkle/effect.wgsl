// Sparkle: twinkling four-point glints on the opaque parts of the layer (text, a logo). Each glint sits at a random
// point of a grid laid on the layer's own box, and shows only where the layer is at least half opaque there; it
// twinkles on its own phase, `speed` times a second, in `color`, and its rays reach `size` px (the padding lets them
// cross the layer's edge). A grid cell is 1.5 rays wide plus `spacing` px, so every spacing spreads the glints out.
// Each glint keeps to the middle third of its cell,
// so it never reaches past its neighbour cells and a pixel looks at its four nearest: five samples a pixel.

// A glint's light at `d` px from its centre, its rays `r` px long: two thin crossed rays and a round core.
fn glint(d: vec2<f32>, r: f32) -> f32 {
    let a = abs(d) / max(r, 0.001);
    let rays = max(0.0, 1.0 - a.x * 8.0 - a.y) + max(0.0, 1.0 - a.y * 8.0 - a.x);
    let core = max(0.0, 1.0 - length(a) * 3.0);
    return clamp(rays + core, 0.0, 1.0);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let base = kl_sample_a(uv);
    let r = max(params.size * kl.px, 1.0);
    let cell = max(params.spacing, 0.0) * kl.px + r * 1.5;
    // Pixels from the layer's own top left, so the grid stays put whatever margin the padding adds.
    let q = kl_content_uv(uv) * kl_content_size();
    let c0 = vec2<i32>(floor(q / cell - vec2<f32>(0.5)));
    var light = 0.0;
    for (var j = 0; j < 2; j++) {
        for (var i = 0; i < 2; i++) {
            let c = c0 + vec2<i32>(i, j);
            let h = kl_hash((u32(c.x) * 73856093u) ^ (u32(c.y) * 19349663u) ^ kl.seed);
            let jitter = vec2<f32>(f32(h & 0xffffu), f32(h >> 16u)) / 65535.0;
            let centre = (vec2<f32>(c) + (vec2<f32>(1.0) + jitter) / 3.0) * cell;
            if (kl_sample_a(kl.content.xy + centre / kl.size).a < 0.5) {
                continue;
            }
            let phase = f32(kl_hash(h) & 0xffffu) / 65535.0;
            // `fract` keeps sin's argument small on a long timeline.
            let twinkle = pow(max(0.0, sin(6.2831853 * fract(kl.time * params.speed + phase))), 8.0);
            light += twinkle * glint(q - centre, r);
        }
    }
    // The glint over the layer, premultiplied.
    let s = clamp(light * params.amount, 0.0, 1.0) * params.color.a;
    return vec4<f32>(params.color.rgb * s + base.rgb * (1.0 - s), s + base.a * (1.0 - s));
}
