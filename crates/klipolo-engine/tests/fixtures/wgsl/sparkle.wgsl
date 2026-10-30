// Sparkle: twinkling four-point glints on the opaque parts of the layer (text, a logo).
fn glint(d: vec2<f32>, r: f32) -> f32 {
    let a = abs(d) / max(r, 0.001);
    let rays = max(0.0, 1.0 - a.x * 8.0 - a.y) + max(0.0, 1.0 - a.y * 8.0 - a.x);
    let core = max(0.0, 1.0 - length(a) * 3.0);
    return clamp(rays + core, 0.0, 1.0);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let base = kl_sample_a(uv);
    let cell = max(params.spacing * kl.px, 2.0);          // glint spacing, in this pass's pixels
    let p = uv * kl.size / cell;
    let c0 = vec2<i32>(floor(p));
    var light = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = c0 + vec2<i32>(i, j);
            let h = kl_hash((u32(c.x) * 73856093u) ^ (u32(c.y) * 19349663u) ^ kl.seed);
            let centre = (vec2<f32>(c) + vec2<f32>(f32(h & 0xffffu), f32(h >> 16u)) / 65535.0) * cell / kl.size;
            if (kl_sample_a(centre).a < 0.5) {
                continue;                                    // only on the text itself
            }
            let phase = f32(kl_hash(h) & 0xffffu) / 65535.0;
            let twinkle = pow(max(0.0, sin(6.2831853 * (kl.time * params.speed + phase))), 8.0);
            light += twinkle * glint(p - (vec2<f32>(c) + vec2<f32>(f32(h & 0xffffu), f32(h >> 16u)) / 65535.0), params.size);
        }
    }
    let s = clamp(light * params.amount, 0.0, 1.0);
    return vec4<f32>(vec3<f32>(s) + base.rgb * (1.0 - s), s + base.a * (1.0 - s));   // white glint over, premultiplied
}
