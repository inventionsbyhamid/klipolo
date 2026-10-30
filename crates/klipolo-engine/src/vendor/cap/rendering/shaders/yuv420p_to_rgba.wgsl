// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/shaders/yuv420p_to_rgba.wgsl
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: BT.709 limited-range coefficients instead of BT.601.

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coords = global_id.xy;
    let dims = textureDimensions(output);

    if (coords.x >= dims.x || coords.y >= dims.y) {
        return;
    }

    let y_raw = textureLoad(y_plane, coords, 0).r;

    let uv_coords = coords / 2;
    let u_dims = textureDimensions(u_plane);
    let v_dims = textureDimensions(v_plane);
    let u_clamped = min(uv_coords, u_dims - vec2<u32>(1, 1));
    let v_clamped = min(uv_coords, v_dims - vec2<u32>(1, 1));
    let u_raw = textureLoad(u_plane, u_clamped, 0).r;
    let v_raw = textureLoad(v_plane, v_clamped, 0).r;

    // BT.709 limited range; Cap used BT.601 coefficients here.
    let y = (y_raw - 16.0 / 255.0) * (255.0 / 219.0);
    let u = u_raw - 128.0 / 255.0;
    let v = v_raw - 128.0 / 255.0;

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
