// A helper named like the host's own functions: refused.
fn kl_glow(uv: vec2<f32>) -> f32 {
    return 1.0 - length(uv - vec2<f32>(0.5));
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(vec3<f32>(kl_glow(uv)), 1.0);
}
