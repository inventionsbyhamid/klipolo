// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/shaders/image.wgsl
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: `flips.x` is set for a mirrored video layer (the shown picture flips before `stored_uv`); `flips.z` is the clockwise turn (0, 90, 180, 270) that makes the texture upright, applied by `stored_uv` (the composite shader's helper) when sampling, for overlay video from phones. `output` (the output's size in pixels), and `vs_main` draws two triangles over the quad's own bounds (its turned corners' box, a pixel round, within the output) where Cap drew one triangle over the whole output, so a title or an image shades its own pixels, not every pixel of the frame; `fs_main` is unchanged, and every pixel outside those bounds drew nothing before (no coverage), so the same pixels are drawn.

struct ImageUniforms {
    center_size: vec4<f32>,
    rotation_opacity_radius: vec4<f32>,
    flips: vec4<f32>,
    output: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: ImageUniforms;
@group(0) @binding(1) var image_texture: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

// Display-oriented uv -> uv in the texture as stored (phone video frames are stored turned).
fn stored_uv(uv: vec2<f32>) -> vec2<f32> {
    let r = uniforms.flips.z;
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

// Klipolo: the quad's bounds in output pixels, two triangles (six vertices).
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let half = uniforms.center_size.zw * 0.5;
    let c = abs(uniforms.rotation_opacity_radius.x);
    let s = abs(uniforms.rotation_opacity_radius.y);
    let reach = vec2<f32>(c * half.x + s * half.y, s * half.x + c * half.y) + vec2<f32>(1.0);
    let lo = clamp(uniforms.center_size.xy - reach, vec2<f32>(0.0), uniforms.output.xy);
    let hi = clamp(uniforms.center_size.xy + reach, vec2<f32>(0.0), uniforms.output.xy);
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let at = mix(lo, hi, corners[index]) / max(uniforms.output.xy, vec2<f32>(1.0)) * 2.0 - vec2<f32>(1.0);
    return vec4<f32>(at.x, -at.y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let offset = position.xy - uniforms.center_size.xy;
    let cosine = uniforms.rotation_opacity_radius.x;
    let sine = uniforms.rotation_opacity_radius.y;
    let local = vec2<f32>(
        cosine * offset.x + sine * offset.y,
        -sine * offset.x + cosine * offset.y,
    );
    let uv = local / uniforms.center_size.zw + vec2<f32>(0.5);
    let flipped_uv = select(uv, vec2<f32>(1.0) - uv, uniforms.flips.xy > vec2<f32>(0.5));
    let color = textureSample(image_texture, image_sampler, stored_uv(flipped_uv));
    let radius = uniforms.rotation_opacity_radius.w;
    let q = abs(local) - uniforms.center_size.zw * 0.5 + vec2<f32>(radius);
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    let feather = max(fwidth(distance), 0.0001);
    let coverage = 1.0 - smoothstep(-feather, 0.0, distance);
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), color, inside)
        * coverage * uniforms.rotation_opacity_radius.z;
}
