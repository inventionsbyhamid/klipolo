// Gradient: from `startColor` to `endColor` along `angle` (0° points right, 90° down), blended in linear light.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let dir = vec2<f32>(cos(params.angle), sin(params.angle));
    let aspect = vec2<f32>(kl.size.x / max(kl.size.y, 1.0), 1.0);
    let p = (uv - vec2<f32>(0.5)) * aspect;
    let reach = abs(dir.x) * aspect.x * 0.5 + abs(dir.y) * 0.5;
    let t = clamp(dot(p, dir) / (2.0 * reach) + 0.5, 0.0, 1.0);
    let rgb = kl_from_linear(mix(kl_to_linear(params.startColor.rgb), kl_to_linear(params.endColor.rgb), vec3<f32>(t)));
    return kl_premultiply(vec4<f32>(rgb, mix(params.startColor.a, params.endColor.a, t)));
}
