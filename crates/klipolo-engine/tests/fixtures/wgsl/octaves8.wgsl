// Octaves: eight layers of value noise (the loop at line 7, which costs most), then a two-tap sheen (line 12).
fn noise(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var v = 0.0; var amp = 0.5; var p = uv * 4.0;
    for (var i = 0; i < 8; i++) {
        v += amp * noise(p);
        p = p * 2.0;
        amp = amp * 0.5;
    }
    var sheen = 0.0; for (var k = 0; k < 2; k++) { sheen += noise(uv + vec2<f32>(f32(k))); }
    return vec4<f32>(vec3<f32>(v + sheen * 0.05), 1.0);
}
