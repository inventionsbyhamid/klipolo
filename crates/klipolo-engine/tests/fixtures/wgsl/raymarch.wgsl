// Too heavy to run: a ray marcher of 200 steps at 1080p (more at higher resolutions, so the loop has no constant
// bound), each step calling a 6-octave noise that also reads 9 texels.
fn corner(i: vec2<f32>) -> f32 {
    let h = kl_hash((u32(i32(i.x)) * 73856093u) ^ (u32(i32(i.y)) * 19349663u));
    return f32(h & 0xffffu) / 65535.0;
}

fn noise(p: vec2<f32>) -> f32 {
    var texels = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            texels += kl_sample_a(fract(p + vec2<f32>(f32(x), f32(y)) * kl_texel())).r;
        }
    }
    var sum = texels / 9.0;
    var amp = 0.5;
    var q = p;
    for (var o = 0; o < 6; o++) {
        let i = floor(q);
        let f = fract(q);
        let u = f * f * (3.0 - 2.0 * f);
        let a = corner(i);
        let b = corner(i + vec2<f32>(1.0, 0.0));
        let c = corner(i + vec2<f32>(0.0, 1.0));
        let d = corner(i + vec2<f32>(1.0, 1.0));
        let ridge = 1.0 - abs(mix(mix(a, b, u.x), mix(c, d, u.x), u.y) * 2.0 - 1.0);
        sum += amp * ridge * ridge;
        q = vec2<f32>(1.6 * q.x - 1.2 * q.y, 1.2 * q.x + 1.6 * q.y) + vec2<f32>(1.7, 9.2);
        amp *= 0.5;
    }
    return sum;
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let steps = max(i32(200.0 * kl.px), 1);
    var t = 0.0;
    var glow = 0.0;
    for (var i = 0; i < steps; i++) {
        let p = vec3<f32>(uv * 2.0 - 1.0, 1.0) * t;
        let d = length(p) - 1.0 - 0.3 * noise(p.xy + vec2<f32>(kl.time, 0.0));
        glow += 0.02 / (0.05 + abs(d));
        t += max(abs(d) * 0.5, 0.01);
    }
    return vec4<f32>(vec3<f32>(clamp(glow, 0.0, 1.0)), 1.0);
}
