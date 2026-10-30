// Vignette: darker towards the corners. `amount` 0–1 sets how dark, `softness` 0–1 how gradual. Centred on the layer's
// own box (`kl.content`), whatever margin a padded filter added.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = kl_sample_a(uv);
    let size = kl_content_size();
    let aspect = vec2<f32>(size.x / max(size.y, 1.0), 1.0);
    let d = length((kl_content_uv(uv) - vec2<f32>(0.5)) * aspect) / length(aspect * 0.5);
    let inner = 1.0 - max(params.softness, 0.01);
    let shade = 1.0 - params.amount * smoothstep(inner, 1.0, d);
    return vec4<f32>(c.rgb * shade, c.a);
}
