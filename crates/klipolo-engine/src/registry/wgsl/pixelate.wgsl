// Pixelate: square cells `size` 1080p pixels wide, each the colour at its centre (for hiding text on screen). The grid
// starts at the layer's own box (`kl.content`), so a padded filter's margin never shifts it.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let cell = max(params.size * kl.px, 1.0);
    let size = kl_content_size();
    let centre = (floor(kl_content_uv(uv) * size / cell) + 0.5) * cell;
    let at = kl.content.xy + centre / size * kl.content.zw;
    return kl_sample_a(min(at, (kl.size - vec2<f32>(0.5)) / kl.size));
}
