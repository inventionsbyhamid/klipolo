// Reads a uniform of its own, which only the host may bind: refused.
@group(2) @binding(0) var<uniform> extra: vec4<f32>;

fn effect(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(uv, 0.0, 1.0) * extra;
}
