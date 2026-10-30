// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/shaders/nv12_to_rgba.wgsl
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: BT.709 limited-range coefficients instead of BT.601.

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var uv_plane: texture_2d<f32>;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coords = global_id.xy;
    let dims = textureDimensions(output);

    if (coords.x >= dims.x || coords.y >= dims.y) {
        return;
    }

    let y_raw = textureLoad(y_plane, coords, 0).r;

    let uv_coords = coords / 2;
    let uv_dims = textureDimensions(uv_plane);
    let uv_clamped = min(uv_coords, uv_dims - vec2<u32>(1, 1));
    let uv_raw = textureLoad(uv_plane, uv_clamped, 0).rg;

    // BT.709 limited range; Cap used BT.601 coefficients here.
    let y = (y_raw - 16.0 / 255.0) * (255.0 / 219.0);
    let u = uv_raw.r - 128.0 / 255.0;
    let v = uv_raw.g - 128.0 / 255.0;

    let r = y + 1.792741 * v;
    let g = y - 0.213249 * u - 0.532909 * v;
    let b = y + 2.112402 * u;

    let color = vec4<f32>(
        clamp(r, 0.0, 1.0),
        clamp(g, 0.0, 1.0),
        clamp(b, 0.0, 1.0),
        1.0
    );

    textureStore(output, coords, color);
}
