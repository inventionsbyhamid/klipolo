// Wipe: an edge `softness` wide sweeps towards `direction`, revealing the incoming picture behind it.
fn transition(uv: vec2<f32>) -> vec4<f32> {
    var x: f32;
    switch params.direction {
        case 0u: { x = 1.0 - uv.x; }
        case 1u: { x = uv.x; }
        case 2u: { x = 1.0 - uv.y; }
        default: { x = uv.y; }
    }
    let s = max(params.softness, 0.001);
    let edge = kl.progress * (1.0 + s);
    let reveal = 1.0 - smoothstep(edge - s, edge, x);
    return mix(kl_sample_a(uv), kl_sample_b(uv), reveal);
}
