// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/shaders/rgba_to_nv12.wgsl
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: the export variant: BT.709 limited range rounded to the nearest code instead of truncated; row strides in bytes with the UV plane at `uv_offset` (256-byte rows for IOSurface plane copies); a 16-bit entry point (`p16`) writes P010 (10-bit 4:2:0) or P210 (10-bit 4:2:2, `chroma_420 == 0`) for HEVC Main 10 and ProRes.

struct Params {
    width: u32,
    height: u32,
    // Bytes per row of both planes.
    stride: u32,
    // Byte offset of the interleaved UV plane.
    uv_offset: u32,
    // 1: one chroma row per two rows (4:2:0); 0: one per row (4:2:2, `p16` only).
    chroma_420: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> yuv_output: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

// BT.709 (Kr 0.2126, Kb 0.0722), limited range: Y 16..235, Cb/Cr 16..240 at 8 bits; ×4 at 10 bits.
fn y_of(c: vec3<f32>) -> f32 {
    return 16.0 + 46.559 * c.r + 156.629 * c.g + 15.812 * c.b;
}

fn u_of(c: vec3<f32>) -> f32 {
    return 128.0 - 25.664 * c.r - 86.336 * c.g + 112.0 * c.b;
}

fn v_of(c: vec3<f32>) -> f32 {
    return 128.0 + 112.0 * c.r - 101.730 * c.g - 10.270 * c.b;
}

fn code8(x: f32) -> u32 {
    return u32(clamp(round(x), 0.0, 255.0));
}

// A 10-bit code in the top bits of 16 (P010/P210 layout).
fn code16(x8: f32) -> u32 {
    return u32(clamp(round(x8 * 4.0), 0.0, 1023.0)) << 6u;
}

fn load(x: u32, y: u32) -> vec3<f32> {
    let c = min(vec2<u32>(x, y), vec2<u32>(params.width - 1u, params.height - 1u));
    return textureLoad(input, c, 0).rgb;
}

// NV12: each invocation converts 4×2 pixels.
@compute @workgroup_size(8, 8)
fn nv12(@builtin(global_invocation_id) id: vec3<u32>) {
    let px = id.x * 4u;
    let py = id.y * 2u;
    if (px >= params.width || py >= params.height) {
        return;
    }
    let p0 = load(px, py);
    let p1 = load(px + 1u, py);
    let p2 = load(px + 2u, py);
    let p3 = load(px + 3u, py);
    let p4 = load(px, py + 1u);
    let p5 = load(px + 1u, py + 1u);
    let p6 = load(px + 2u, py + 1u);
    let p7 = load(px + 3u, py + 1u);

    let row0 = code8(y_of(p0)) | (code8(y_of(p1)) << 8u) | (code8(y_of(p2)) << 16u) | (code8(y_of(p3)) << 24u);
    yuv_output[(py * params.stride + px) / 4u] = row0;
    if (py + 1u < params.height) {
        let row1 = code8(y_of(p4)) | (code8(y_of(p5)) << 8u) | (code8(y_of(p6)) << 16u) | (code8(y_of(p7)) << 24u);
        yuv_output[((py + 1u) * params.stride + px) / 4u] = row1;
    }

    let left = (p0 + p1 + p4 + p5) * 0.25;
    let right = (p2 + p3 + p6 + p7) * 0.25;
    let uv = code8(u_of(left)) | (code8(v_of(left)) << 8u) | (code8(u_of(right)) << 16u) | (code8(v_of(right)) << 24u);
    yuv_output[(params.uv_offset + id.y * params.stride + px) / 4u] = uv;
}

// P010 / P210: each invocation converts 2×2 pixels (one 32-bit word holds two 16-bit samples).
@compute @workgroup_size(8, 8)
fn p16(@builtin(global_invocation_id) id: vec3<u32>) {
    let px = id.x * 2u;
    let py = id.y * 2u;
    if (px >= params.width || py >= params.height) {
        return;
    }
    let p0 = load(px, py);
    let p1 = load(px + 1u, py);
    let p2 = load(px, py + 1u);
    let p3 = load(px + 1u, py + 1u);

    yuv_output[(py * params.stride + px * 2u) / 4u] = code16(y_of(p0)) | (code16(y_of(p1)) << 16u);
    if (py + 1u < params.height) {
        yuv_output[((py + 1u) * params.stride + px * 2u) / 4u] = code16(y_of(p2)) | (code16(y_of(p3)) << 16u);
    }

    if (params.chroma_420 == 1u) {
        let c = (p0 + p1 + p2 + p3) * 0.25;
        yuv_output[(params.uv_offset + id.y * params.stride + px * 2u) / 4u] = code16(u_of(c)) | (code16(v_of(c)) << 16u);
    } else {
        let top = (p0 + p1) * 0.5;
        let bottom = (p2 + p3) * 0.5;
        yuv_output[(params.uv_offset + py * params.stride + px * 2u) / 4u] = code16(u_of(top)) | (code16(v_of(top)) << 16u);
        if (py + 1u < params.height) {
            yuv_output[(params.uv_offset + (py + 1u) * params.stride + px * 2u) / 4u] = code16(u_of(bottom)) | (code16(v_of(bottom)) << 16u);
        }
    }
}
