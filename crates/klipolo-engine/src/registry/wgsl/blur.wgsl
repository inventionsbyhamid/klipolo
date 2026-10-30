// Blur: a Gaussian (σ = radius / 2, out to `radius` 1080p pixels). It is separable: the host draws `effect` across
// and then down (blur's second, host-run pass with the same settings). Each bilinear fetch weighs two neighbouring
// texels (the linear-sampling trick), so a pass costs about radius / 2 fetches each side, and taps stay a texel
// apart (strided once the radius passes 32 pixels) so sharp edges don't ghost. Neighbouring weights step by
// constant factors: no transcendental maths per tap.
fn blur_along(uv: vec2<f32>, axis: vec2<f32>) -> vec4<f32> {
    let r = params.radius * kl.px;
    if (r < 0.5) {
        return kl_sample_a(uv);
    }
    let stride = max(r / 32.0, 1.0);
    let sigma = 0.5 * r / stride;
    let taps = u32(ceil(r / stride));
    let k2 = 1.0 / (2.0 * sigma * sigma);
    // w(k + 1) / w(k) = exp(-(2k + 1) k2): it starts at exp(-k2) and shrinks by exp(-2 k2) per texel.
    let fall = exp(-2.0 * k2);
    var ratio = exp(-k2);
    var w = 1.0;
    let step = axis * kl_texel() * stride;
    var sum = kl_sample_a(uv);
    var total = 1.0;
    for (var k = 1u; k <= taps; k = k + 2u) {
        let w1 = w * ratio;
        ratio = ratio * fall;
        let w2 = w1 * ratio;
        ratio = ratio * fall;
        let pair = w1 + w2;
        let at = (f32(k) * w1 + f32(k + 1u) * w2) / pair;
        sum = sum + (kl_sample_a(uv + step * at) + kl_sample_a(uv - step * at)) * pair;
        total = total + 2.0 * pair;
        w = w2;
    }
    return sum / total;
}
