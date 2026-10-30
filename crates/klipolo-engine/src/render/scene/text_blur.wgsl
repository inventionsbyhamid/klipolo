// A scene text's blurring pieces (drawn small, as `registry/wgsl/blur.wgsl` draws its own):
// three passes over each piece's own region of two scratch targets, a texel every `step` atlas texels (its cell grown
// by its margin each way). `fs_sample` takes each texel as the mean of its step × step atlas texels (none outside the
// cell); `fs_across` then `fs_down` blur that by the piece's Gaussian, a few bilinear fetches each weighing two texels
// at once, the weights stepping by constant factors (no `exp()` a tap). The text pass reads the result back with one
// bilinear fetch (`text.wgsl`). R the fill's coverage, G the silhouette, as the atlas has them. Nothing reads outside a
// region: a tap past its edge counts as transparent.

struct Slot {
    // The cell's uv rect in the atlas.
    cell: vec4<f32>,
    // Its region of the scratch targets, pixels: left, top, right, bottom.
    region: vec4<f32>,
    // Its step (atlas texels a texel, a power of two), its margin past the cell (atlas texels, whole steps), its taps
    // each way.
    misc: vec4<f32>,
    // Its Gaussian's first ratio of neighbouring weights, and the factor each next one falls by.
    weights: vec4<f32>,
    // The scratch targets' size, and the atlas's.
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> slots: array<Slot, 24>;
// What the pass reads: the atlas's masks (`fs_sample`), else the scratch target the pass before drew.
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var blur_sampler: sampler;

struct BlurVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) slot: u32,
};

@vertex
fn vs_blur(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> BlurVaryings {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let s = slots[i];
    let p = mix(s.region.xy, s.region.zw, corners[v % 6u]);
    let size = max(s.size.xy, vec2<f32>(1.0));
    var out: BlurVaryings;
    out.position = vec4<f32>(p.x / size.x * 2.0 - 1.0, 1.0 - p.y / size.y * 2.0, 0.0, 1.0);
    out.slot = i;
    return out;
}

// The first pass: this texel is the mean of its step × step box of the atlas, read two by two (each bilinear fetch at
// the corner four texels share is their mean). The margin is whole steps, so a box starts on the cell's edge or a whole
// box from it: a pair never starts outside the cell, and one that ends past its right or bottom edge reads the blank
// texel every cell keeps round its ink.
@fragment
fn fs_sample(v: BlurVaryings) -> @location(0) vec4<f32> {
    let s = slots[v.slot];
    let step = max(s.misc.x, 1.0);
    let atlas = s.size.zw;
    let cell = round(s.cell * vec4<f32>(atlas, atlas));
    let ij = floor(v.position.xy - s.region.xy);
    let origin = cell.xy - vec2<f32>(s.misc.y) + ij * step;
    if step < 1.5 {
        if all(origin >= cell.xy) && all(origin < cell.zw) {
            return vec4<f32>(textureLoad(source, vec2<i32>(origin), 0).rg, 0.0, 1.0);
        }
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let half = i32(step) / 2;
    let lo = max(vec2<i32>(0), vec2<i32>(ceil((cell.xy - origin) * 0.5)));
    let hi = min(vec2<i32>(half), vec2<i32>(ceil((cell.zw - origin) * 0.5)));
    var sum = vec2<f32>(0.0);
    for (var b = lo.y; b < hi.y; b = b + 1) {
        for (var a = lo.x; a < hi.x; a = a + 1) {
            let at = origin + vec2<f32>(f32(2 * a) + 1.0, f32(2 * b) + 1.0);
            sum = sum + textureSampleLevel(source, blur_sampler, at / atlas, 0.0).rg;
        }
    }
    return vec4<f32>(sum / f32(half * half), 0.0, 1.0);
}

// Whether the texel whose centre is `q` lies in the region `r`.
fn inside(q: vec2<f32>, r: vec4<f32>) -> bool {
    return all(q >= r.xy) && all(q < r.zw);
}

// The scratch target the pass reads, at pixel `q` (bilinear).
fn read(q: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    return textureSampleLevel(source, blur_sampler, q / size, 0.0).rg;
}

// This texel blurred along `axis` by the piece's Gaussian: the middle texel, then each pair of taps out to its reach as
// one fetch between them, weighted by both (a pair whose outer tap is past the region reads its inner texel alone; one
// whose inner tap is past it reads nothing). Taps past the region weigh in as transparent.
fn gaussian(v: BlurVaryings, axis: vec2<f32>) -> vec4<f32> {
    let s = slots[v.slot];
    let size = max(s.size.xy, vec2<f32>(1.0));
    let p = v.position.xy;
    let taps = u32(s.misc.z);
    var sum = read(p, size);
    var total = 1.0;
    var w = 1.0;
    var ratio = s.weights.x;
    let fall = s.weights.y;
    for (var k = 1u; k <= taps; k = k + 2u) {
        let w1 = w * ratio;
        ratio = ratio * fall;
        var w2 = w1 * ratio;
        ratio = ratio * fall;
        if k + 1u > taps {
            w2 = 0.0;
        }
        let pair = w1 + w2;
        let at = (f32(k) * w1 + f32(k + 1u) * w2) / pair;
        for (var side = -1.0; side < 2.0; side = side + 2.0) {
            let near = p + axis * side * f32(k);
            if !inside(near, s.region) {
                continue;
            }
            if inside(p + axis * side * f32(k + 1u), s.region) {
                sum = sum + read(p + axis * side * at, size) * pair;
            } else {
                sum = sum + read(near, size) * w1;
            }
        }
        total = total + 2.0 * pair;
        w = w2;
    }
    return vec4<f32>(sum / total, 0.0, 1.0);
}

@fragment
fn fs_across(v: BlurVaryings) -> @location(0) vec4<f32> {
    return gaussian(v, vec2<f32>(1.0, 0.0));
}

@fragment
fn fs_down(v: BlurVaryings) -> @location(0) vec4<f32> {
    return gaussian(v, vec2<f32>(0.0, 1.0));
}
