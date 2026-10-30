// Overlay blend: one read-below pass over the frame's pixels a layer covers (`kl.size` from
// `params.origin`). `kl_a` is the frame drawn so far, read texel for texel; `kl_b` is the layer's own texture, placed
// as the image quad places it (`image.wgsl`: centre, size, rotation, rounding, opacity, the mirror and the stored
// turn, all in frame pixels). Per channel, on the colour below `a` and the layer's colour `b` (unpremultiplied):
// mix(2·a·b, 1 − 2·(1 − a)·(1 − b), step(0.5, a)), mixed over `a` by the layer's alpha. The frame below is opaque, so
// the result is too, and the compositor draws it back in place.

fn stored_uv(uv: vec2<f32>) -> vec2<f32> {
    let r = params.turn;
    if r > 45.0 && r < 135.0 {
        return vec2<f32>(uv.y, 1.0 - uv.x);
    }
    if r >= 135.0 && r < 225.0 {
        return vec2<f32>(1.0 - uv.x, 1.0 - uv.y);
    }
    if r >= 225.0 && r < 315.0 {
        return vec2<f32>(1.0 - uv.y, uv.x);
    }
    return uv;
}

fn transition(uv: vec2<f32>) -> vec4<f32> {
    let position = params.origin + uv * kl.size;
    let below = textureLoad(kl_a, vec2<i32>(floor(position)), 0);
    let offset = position - params.center;
    let local = vec2<f32>(
        params.cosine * offset.x + params.sine * offset.y,
        -params.sine * offset.x + params.cosine * offset.y,
    );
    let luv = local / params.size + vec2<f32>(0.5);
    // A mirrored layer flips as shown, before its stored turn, as the image quad's `flips.x` does.
    let shown = select(luv, vec2<f32>(1.0 - luv.x, luv.y), params.mirror > 0.5);
    // Implicit derivatives pick the mip level an image's texture needs at its drawn size, as the image quad does.
    let colour = textureSample(kl_b, kl_samp, stored_uv(clamp(shown, vec2<f32>(0.0), vec2<f32>(1.0))));
    let radius = params.rounding;
    let q = abs(local) - params.size * 0.5 + vec2<f32>(radius);
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    let coverage = 1.0 - smoothstep(-1.0, 0.0, distance);
    let inside = all(luv >= vec2<f32>(0.0)) && all(luv <= vec2<f32>(1.0));
    let layer = select(vec4<f32>(0.0), colour, inside) * coverage * params.opacity;
    let alpha = layer.a;
    let b = select(vec3<f32>(0.0), layer.rgb / max(alpha, 1e-6), alpha > 0.0);
    let a = below.rgb;
    let overlay = mix(2.0 * a * b, vec3<f32>(1.0) - 2.0 * (vec3<f32>(1.0) - a) * (vec3<f32>(1.0) - b), step(vec3<f32>(0.5), a));
    return vec4<f32>(mix(a, overlay, alpha), 1.0);
}
