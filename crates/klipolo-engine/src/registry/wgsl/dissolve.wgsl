// Dissolve: the outgoing picture fades into the incoming one (premultiplied mix, eased).
fn transition(uv: vec2<f32>) -> vec4<f32> {
    return mix(kl_sample_a(uv), kl_sample_b(uv), kl_ease_in_out(kl.progress));
}
