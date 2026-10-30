// Water waves: crossing swells lit from above, foam on the crests.
fn swell(p: vec2<f32>, t: f32) -> f32 {
    var h = 0.0;
    var amp = 0.5;
    var freq = params.density;
    var dir = vec2<f32>(1.0, 0.3);
    for (var i = 0; i < 5; i++) {
        h += amp * sin(dot(p, normalize(dir)) * freq + t * params.speed * (1.0 + 0.4 * f32(i)));
        dir = vec2<f32>(0.8 * dir.x - 0.6 * dir.y, 0.6 * dir.x + 0.8 * dir.y);
        amp *= 0.55;
        freq *= 1.8;
    }
    return h;
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let p = vec2<f32>(uv.x * kl.size.x / max(kl.size.y, 1.0), uv.y) * 4.0;
    let e = 0.02;
    let h = swell(p, kl.time);
    let dx = swell(p + vec2<f32>(e, 0.0), kl.time) - h;
    let dy = swell(p + vec2<f32>(0.0, e), kl.time) - h;
    let n = normalize(vec3<f32>(-dx, -dy, e * 3.0));
    let shade = clamp(dot(n, normalize(vec3<f32>(0.3, -0.5, 0.8))), 0.0, 1.0);
    var c = mix(kl_to_linear(params.deepColor.rgb), kl_to_linear(params.shallowColor.rgb),
                vec3<f32>(clamp(0.5 + 0.5 * h, 0.0, 1.0))) * (0.6 + 0.4 * shade);
    c = mix(c, vec3<f32>(0.9), smoothstep(0.55, 0.8, h) * params.foam);
    return vec4<f32>(kl_from_linear(c), 1.0);
}
