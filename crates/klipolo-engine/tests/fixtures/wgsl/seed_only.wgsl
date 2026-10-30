// A tint picked once per layer: reads only kl.seed, so every frame is the same.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let h = kl_hash(kl.seed);
    let tint = vec3<f32>(f32(h & 0xffu), f32((h >> 8u) & 0xffu), f32((h >> 16u) & 0xffu)) / 255.0;
    return vec4<f32>(tint * uv.x, 1.0);
}
