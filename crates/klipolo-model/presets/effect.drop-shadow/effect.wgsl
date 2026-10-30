// Drop shadow: the shape of the layer in `color`, moved `distance` px towards `angle` (0° points right, 90° down) and
// softened, laid under the layer. The shadow's blur reaches `softness` of half the distance, so all of it stays within
// 1.5 × the distance of the layer's box: the padding's margin, which is why a soft shadow needs some distance. Eight
// taps soften it, on a ring that widens as it turns, 135° from one tap to the next so the eight point every way and
// the shadow softens as much on every side, crowded towards its centre; each pixel of a 4 × 4 block turns the ring a
// further sixteenth of a full turn (ordered dithering), so a wide blur's edge reads as a fine even texture rather than
// grain. With the layer's own pixel: nine samples.

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let base = kl_sample_a(uv);
    let dist = params.distance * kl.px;
    let at = uv - vec2<f32>(cos(params.angle), sin(params.angle)) * dist / kl.size;
    let spread = clamp(params.softness, 0.0, 1.0) * 0.5 * dist / kl.size;
    // This pixel's place in its 4 × 4 Bayer block: 0–15.
    let q = vec2<u32>(uv * kl.size) & vec2<u32>(3u);
    let m = q.x ^ q.y;
    let order = f32(((m & 1u) << 3u) | ((q.y & 1u) << 2u) | (m & 2u) | ((q.y & 2u) >> 1u)) / 16.0;
    var sum = 0.0;
    for (var i = 0; i < 8; i++) {
        let k = f32(i);
        let a = 6.2831853 * order + 2.3561945 * k;
        let far = (k + 0.5) / 8.0;
        sum += kl_sample_a(at + vec2<f32>(cos(a), sin(a)) * spread * far).a;
    }
    let shade = sum / 8.0 * params.color.a;
    let under = vec4<f32>(params.color.rgb * shade, shade);
    return base + under * (1.0 - base.a);
}
