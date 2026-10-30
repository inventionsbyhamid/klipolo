// Light leak: two warm glows drifting slowly in from the edges of the frame, as light leaking into an old film camera.
// `warmColor` and `softColor` tint them, `size` sets how far they spread (a share of the frame's shorter side),
// `intensity` how bright they are and `speed` how fast they drift. Black elsewhere, for a layer laid over the video
// with the screen blend (where black changes nothing). Added in linear light.

// A soft round glow of radius `r` around `c`: 1 at its centre.
fn leak(p: vec2<f32>, c: vec2<f32>, r: f32) -> f32 {
    let d = (p - c) / max(r, 0.001);
    return exp(-dot(d, d));
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let side = max(min(kl.size.x, kl.size.y), 1.0);
    let edge = 0.5 * kl.size / side;
    let p = (uv - vec2<f32>(0.5)) * kl.size / side;
    // Each layer drifts its own way.
    let t = kl.time * params.speed + f32(kl.seed & 0xffffu) / 65535.0 * 6.2831853;
    let r = max(params.size, 0.05);
    let a = vec2<f32>(-edge.x * (0.8 + 0.25 * sin(t * 0.31)), edge.y * 0.6 * sin(t * 0.23 + 1.3));
    let b = vec2<f32>(edge.x * (0.7 + 0.3 * sin(t * 0.27 + 2.0)), -edge.y * (0.5 + 0.4 * sin(t * 0.19 + 0.4)));
    let breathe = 0.8 + 0.2 * sin(t * 0.53);
    let warm = kl_to_linear(params.warmColor.rgb) * (leak(p, a, r * (0.9 + 0.15 * sin(t * 0.41))) * breathe);
    let soft = kl_to_linear(params.softColor.rgb) * (leak(p, b, r * 0.7) * (1.2 - 0.5 * breathe));
    let rgb = kl_from_linear((warm + soft) * params.intensity);
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
