// Rings, one per step: the steps setting bounds the loop.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = 0.0;
    for (var i = 0; i < i32(params.steps); i++) {
        c += 0.5 + 0.5 * sin(length(uv - vec2<f32>(0.5)) * 40.0 - f32(i));
    }
    return vec4<f32>(vec3<f32>(c / max(f32(params.steps), 1.0)), 1.0);
}
