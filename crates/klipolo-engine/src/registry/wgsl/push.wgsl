// Push: the incoming picture pushes the outgoing one out towards `direction` (0 left, 1 right, 2 up, 3 down).
fn push_dir() -> vec2<f32> {
    switch params.direction {
        case 0u: { return vec2<f32>(-1.0, 0.0); }
        case 1u: { return vec2<f32>(1.0, 0.0); }
        case 2u: { return vec2<f32>(0.0, -1.0); }
        default: { return vec2<f32>(0.0, 1.0); }
    }
}

fn inside(uv: vec2<f32>) -> bool {
    return all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
}

fn transition(uv: vec2<f32>) -> vec4<f32> {
    let shift = push_dir() * kl_ease_in_out(kl.progress);
    let a_uv = uv - shift;
    let b_uv = uv - shift + push_dir();
    var c = vec4<f32>(0.0);
    if (inside(a_uv)) {
        c = kl_sample_a(a_uv);
    }
    if (inside(b_uv)) {
        let b = kl_sample_b(b_uv);
        c = b + c * (1.0 - b.a);
    }
    return c;
}
