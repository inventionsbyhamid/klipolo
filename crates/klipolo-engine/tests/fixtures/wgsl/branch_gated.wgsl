// Detail, a fancy pass and the third mode each switch a loop on.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = vec4<f32>(uv, 0.0, 1.0);
    if (params.detail > 0.5) {
        for (var i = 0; i < 48; i++) {
            c += kl_sample_a(uv + vec2<f32>(f32(i), 0.0) * kl_texel()) / 48.0;
        }
    }
    if (params.fancy == 1u) {
        for (var i = 0; i < 32; i++) {
            c = vec4<f32>(c.rgb * 0.99 + vec3<f32>(sin(f32(i) + uv.x) * 0.01), c.a);
        }
    }
    switch (params.mode) {
        case 2u: {
            for (var i = 0; i < 16; i++) {
                c = vec4<f32>(c.gbr, c.a);
            }
        }
        default: {}
    }
    return clamp(c, vec4<f32>(0.0), vec4<f32>(1.0));
}
