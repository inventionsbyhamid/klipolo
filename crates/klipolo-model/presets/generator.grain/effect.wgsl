// Film grain: fine grain centred on mid-grey that changes every frame, for a layer laid over the video with the
// overlay blend (where grey changes nothing). `amount` is how strong it is, `size` a grain's size in px and `chroma`
// how much of it is coloured. Value noise on a grid of `size`, eased between its points so grains larger than a pixel
// are soft rather than square; a new pattern at each of its 24 steps a second.

// The pattern `salt`'s value at grid point `g`: 0–1.
fn grain_at(g: vec2<i32>, salt: u32) -> f32 {
    return f32(kl_hash(u32(g.x) ^ kl_hash(u32(g.y) ^ salt)) >> 8u) / 16777215.0;
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let cell = max(params.size * kl.px, 1.0);
    let q = uv * kl.size / cell;
    let g = vec2<i32>(floor(q));
    let f = q - floor(q);
    let w = f * f * (3.0 - 2.0 * f);
    let salt = kl_hash(kl.seed ^ (kl.frame * 0x9e3779b9u));
    let top = mix(grain_at(g, salt), grain_at(g + vec2<i32>(1, 0), salt), w.x);
    let bottom = mix(grain_at(g + vec2<i32>(0, 1), salt), grain_at(g + vec2<i32>(1, 1), salt), w.x);
    let mono = mix(top, bottom, w.y) - 0.5;
    let h = kl_hash(u32(g.x) ^ kl_hash(u32(g.y) ^ salt ^ 0x5bd1e995u));
    let tint = vec3<f32>(f32(h & 0x3ffu), f32((h >> 10u) & 0x3ffu), f32((h >> 20u) & 0x3ffu)) / 1023.0 - 0.5;
    let v = 0.5 + params.amount * mix(vec3<f32>(mono), tint, clamp(params.chroma, 0.0, 1.0));
    return vec4<f32>(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
