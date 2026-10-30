// Flashes on every other frame: reads kl.frame itself.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let on = f32(kl.frame % 2u);
    return vec4<f32>(uv * on, 0.0, 1.0);
}
