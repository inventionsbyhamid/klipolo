// Glow: a soft light of `color` around the shape of the layer, reaching `radius` px past it (the padding gives it room
// past the layer's box), `strength` times as dense, laid under the layer. The layer's alpha is blurred with nine taps:
// its own pixel and eight on a ring that widens as it turns, 135° from one tap to the next so the eight point every
// way, crowded towards the pixel (their distances grow evenly, not with the disc's area), so the glow is bright near
// the shape and fades in a long soft tail, as far on every side. Each pixel of a 4 × 4 block turns the ring a further
// sixteenth of a full turn (ordered dithering), so a block reads 128 taps between them: a fine even texture rather than
// the grain of random turns, and no ghost copies of the shape.

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let base = kl_sample_a(uv);
    let reach = params.radius * kl.px / kl.size;
    // This pixel's place in its 4 × 4 Bayer block: 0–15.
    let q = vec2<u32>(uv * kl.size) & vec2<u32>(3u);
    let m = q.x ^ q.y;
    let order = f32(((m & 1u) << 3u) | ((q.y & 1u) << 2u) | (m & 2u) | ((q.y & 2u) >> 1u)) / 16.0;
    var sum = base.a;
    for (var i = 0; i < 8; i++) {
        let k = f32(i);
        let a = 6.2831853 * order + 2.3561945 * k;
        let far = (k + 0.5) / 8.0;
        sum += kl_sample_a(uv + vec2<f32>(cos(a), sin(a)) * reach * far).a;
    }
    let glow = clamp(sum / 9.0 * 1.5 * params.strength, 0.0, 1.0) * params.color.a;
    let under = vec4<f32>(params.color.rgb * glow, glow);
    return base + under * (1.0 - base.a);
}
