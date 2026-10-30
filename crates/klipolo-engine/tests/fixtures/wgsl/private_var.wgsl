// Keeps state outside its functions: refused.
var<private> counter: f32;

fn effect(uv: vec2<f32>) -> vec4<f32> {
    counter = counter + uv.x;
    return vec4<f32>(counter, uv.y, 0.0, 1.0);
}
