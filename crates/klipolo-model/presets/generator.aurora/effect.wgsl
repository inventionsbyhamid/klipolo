// Aurora: three curtains of northern light drifting over a night sky. Each curtain's lower edge waves with smooth value
// noise, and its light fades upward in vertical rays; `colorA`, `colorB` and `colorC` light them, added in linear light
// over `skyColor`. `speed` sets how fast they drift, `brightness` how strongly they shine. No texture reads, and
// smooth, so it draws at half size.

// Smooth noise along one axis: a random height (0–1) at each whole number, eased between them.
fn curtain_noise(x: f32, salt: u32) -> f32 {
    let i = floor(x);
    let f = x - i;
    let n = u32(i32(i));
    let a = f32(kl_hash(n ^ salt) >> 8u) / 16777215.0;
    let b = f32(kl_hash((n + 1u) ^ salt) >> 8u) / 16777215.0;
    return mix(a, b, f * f * (3.0 - 2.0 * f));
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    // Across in the frame's shorter side, so the curtains keep their size in every format.
    let x = uv.x * kl.size.x / max(min(kl.size.x, kl.size.y), 1.0);
    let t = kl.time * params.speed;
    var light = vec3<f32>(0.0);
    for (var i = 0; i < 3; i++) {
        let k = f32(i);
        let salt = kl.seed ^ (0x9e3779b9u * u32(i + 1));
        // The curtain's lower edge: a slow, wide drift plus a finer ripple.
        let wide = curtain_noise(x * 1.1 + t * (0.05 + 0.02 * k), salt) - 0.5;
        let fine = curtain_noise(x * 3.3 - t * 0.09, salt ^ 0x68bc21ebu) - 0.5;
        let d = uv.y - (0.42 + 0.1 * k + 0.18 * wide + 0.07 * fine);
        // Below the edge the light ends sharply; above it, it fades over a long way.
        let fall = select(exp(d * 6.0), exp(-d * d * 900.0), d > 0.0);
        let rays = 0.45 + 0.55 * curtain_noise(x * 22.0 + t * 0.25 + k * 13.0, salt ^ 0x2545f491u);
        let tint = select(select(params.colorC, params.colorB, i == 1), params.colorA, i == 0);
        light += kl_to_linear(tint.rgb) * (fall * rays * (1.0 - 0.25 * k));
    }
    // The sky, a little lighter towards the horizon.
    let sky = kl_to_linear(params.skyColor.rgb) * (0.55 + 0.45 * uv.y);
    let rgb = kl_from_linear(sky + light * (0.8 * params.brightness));
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
