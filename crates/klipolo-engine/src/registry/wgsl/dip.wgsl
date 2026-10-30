// Dip to colour: the outgoing picture fades to `color`, then the incoming one fades in from it.
fn transition(uv: vec2<f32>) -> vec4<f32> {
    let colour = kl_premultiply(params.color);
    if (kl.progress < 0.5) {
        return mix(kl_sample_a(uv), colour, kl_ease_in_out(kl.progress * 2.0));
    }
    return mix(colour, kl_sample_b(uv), kl_ease_in_out(kl.progress * 2.0 - 1.0));
}
