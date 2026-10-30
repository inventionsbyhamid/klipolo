// Brackets 41 deep (the function's brace and 40 parentheses): refused before naga parses it.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let x = ((((((((((((((((((((((((((((((((((((((((uv.x))))))))))))))))))))))))))))))))))))))));
    return vec4<f32>(x, 0.0, 0.0, 1.0);
}
