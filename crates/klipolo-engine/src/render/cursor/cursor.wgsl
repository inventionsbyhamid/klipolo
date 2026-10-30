// The cursor pass: each instance is a quad drawn from one ring of 64-byte entries, indexed by
// `instance_index` (one bind group, no vertex buffer). An instance names its slot's header entry: the footage's box
// in pixels, its rounded corners and its shape mask, and the target's size. Kinds:
// - 0 shape: the take's image from the atlas, trilinear when minified (the level from the drawn size, at most 4),
//   bilinear when magnified, with a soft shadow from its alpha 0.08 heights down at 25 %;
// - 1 dot: a circle with a 1.5 px white outline; 2 arrow: a 7-point polygon, white with a black outline. Both are
//   signed distances in pixels (the quad is never turned, so a local unit is exactly `height` pixels: what `fwidth`
//   would give), so they are sharp at any size;
// - 3 ripple: a ring 0.12 of its radius wide, in the click colour.
// A smear (motion blur) averages 1 + N taps along its vector, N = clamp(length / 2, 1, 16); a pixel evaluates only the
// taps that land on the look (`tap_range`), as the rest add nothing. Colours are premultiplied and blended over.

struct Entry {
    // kind, the header's index, opacity (alpha for a ripple), unused
    head: vec4<f32>,
    // cursor: hotspot (px), height, width (px); ripple: centre (px), radius (px), unused
    place: vec4<f32>,
    // shape: the image's rectangle in the atlas (u0, v0, u1, v1); ripple: RGB 0–1
    image: vec4<f32>,
    // cursor: the hotspot in its image (0–1), the smear (px)
    more: vec4<f32>,
};
// A header: head = the box (x0, y0, x1, y1, px); place = corner radius (px), mask shape (−1 none), feather (px),
// mask corner (px); image = the mask's centre and half size (px); more = the target's size.

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(0) @binding(2) var<storage, read> ring: array<Entry>;

const SHAPE: u32 = 0u;
const DOT: u32 = 1u;
const RIPPLE: u32 = 3u;
const ATLAS_SIDE: f32 = 1024.0;
const MAX_LOD: f32 = 4.0;
const DROP: f32 = 0.08;
const SHADOW: f32 = 0.25;
const RING: f32 = 0.12;
const DOT_LINE: f32 = 1.5;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) index: u32,
    @location(1) @interpolate(flat) lod: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
    let e = ring[ii];
    let target_px = ring[u32(e.head.y)].more.xy;
    var lo: vec2<f32>;
    var hi: vec2<f32>;
    var lod = 0.0;
    if (u32(e.head.x) == RIPPLE) {
        let reach = vec2<f32>(e.place.z * (1.0 + RING) + 2.0);
        lo = e.place.xy - reach;
        hi = e.place.xy + reach;
    } else {
        let size = vec2<f32>(e.place.w, e.place.z);
        let origin = e.place.xy - e.more.xy * size;
        let margin = vec2<f32>(2.0 + 0.1 * size.y);
        lo = origin - abs(e.more.zw) * 0.5 - margin;
        hi = origin + size + abs(e.more.zw) * 0.5 + vec2<f32>(0.0, DROP * size.y) + margin;
        if (u32(e.head.x) == SHAPE) {
            let texels = (e.image.w - e.image.y) * ATLAS_SIDE;
            lod = clamp(log2(max(texels, 1.0) / max(size.y, 0.001)), 0.0, MAX_LOD);
        }
    }
    // Two triangles: corners (0,0) (1,0) (0,1) (0,1) (1,0) (1,1).
    let corner = vec2<f32>(f32((0x32u >> vi) & 1u), f32((0x2Cu >> vi) & 1u));
    let px = mix(lo, hi, corner);
    var out: Out;
    out.position = vec4<f32>(px.x / target_px.x * 2.0 - 1.0, 1.0 - px.y / target_px.y * 2.0, 0.0, 1.0);
    out.index = ii;
    out.lod = lod;
    return out;
}

// Signed distance from `q` (from a box's centre) to a box of half size `h` with corners of radius `r`, negative inside
// (as `mask.wgsl`).
fn box_sdf(q: vec2<f32>, h: vec2<f32>, r: f32) -> f32 {
    let e = abs(q) - h + vec2<f32>(r);
    return length(max(e, vec2<f32>(0.0))) + min(max(e.x, e.y), 0.0) - r;
}

// How much of the pixel at `p` the footage's rounded box and its shape mask keep.
fn kept(p: vec2<f32>, h: Entry) -> f32 {
    var cover = 1.0;
    if (h.place.x > 0.0) {
        let half = (h.head.zw - h.head.xy) * 0.5;
        cover = clamp(0.5 - box_sdf(p - h.head.xy - half, half, h.place.x), 0.0, 1.0);
    }
    if (h.place.y >= 0.0) {
        let d = box_sdf(p - h.image.xy, h.image.zw, h.place.w);
        cover *= 1.0 - smoothstep(-h.place.z - 0.5, 0.5, d);
    }
    return cover;
}

// One edge of a polygon (from `a`, the vertex before it `b`): the squared distance from `p` to the edge, and -1 when
// a ray from `p` crosses it (else 1).
fn edge(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let e = b - a;
    let w = p - a;
    let near = w - e * clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
    let c = vec3<bool>(p.y >= a.y, p.y < b.y, e.x * w.y > e.y * w.x);
    return vec2<f32>(dot(near, near), select(1.0, -1.0, all(c) || all(!c)));
}

// Klipolo's arrow, its tip at the origin, one unit high: the signed distance to its outline (negative inside). Its seven
// edges written out, so no compiler keeps the vertices in an indexed array.
fn arrow(p: vec2<f32>) -> f32 {
    let v0 = vec2<f32>(0.0, 0.0);
    let v1 = vec2<f32>(0.0, 0.78);
    let v2 = vec2<f32>(0.19, 0.605);
    let v3 = vec2<f32>(0.33, 0.92);
    let v4 = vec2<f32>(0.455, 0.865);
    let v5 = vec2<f32>(0.32, 0.56);
    let v6 = vec2<f32>(0.57, 0.56);
    let e0 = edge(p, v0, v6);
    let e1 = edge(p, v1, v0);
    let e2 = edge(p, v2, v1);
    let e3 = edge(p, v3, v2);
    let e4 = edge(p, v4, v3);
    let e5 = edge(p, v5, v4);
    let e6 = edge(p, v6, v5);
    let d = min(min(min(e0.x, e1.x), min(e2.x, e3.x)), min(min(e4.x, e5.x), e6.x));
    return e0.y * e1.y * e2.y * e3.y * e4.y * e5.y * e6.y * sqrt(d);
}

// `p` (in arrow heights) is within `grow` of the arrow's own box (0–0.57 across, 0–0.92 down).
fn near_arrow(p: vec2<f32>, grow: f32) -> bool {
    return all(p >= vec2<f32>(-grow)) && all(p <= vec2<f32>(0.57, 0.92) + vec2<f32>(grow));
}

// The image at `uv` (0–1 over it), nothing outside it; never a neighbour's texel at the level read.
fn image_at(e: Entry, uv: vec2<f32>, lod: f32) -> vec4<f32> {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let half = vec2<f32>(0.5 * exp2(ceil(lod)) / ATLAS_SIDE);
    // An image narrower than two texels at this level samples its middle: the bounds never cross.
    let mid = 0.5 * (e.image.xy + e.image.zw);
    let at = clamp(mix(e.image.xy, e.image.zw, uv), min(e.image.xy + half, mid), max(e.image.zw - half, mid));
    return textureSampleLevel(atlas, linear, at, lod);
}

// One tap of a cursor look at `q` (pixels), its shadow under it.
fn look(kind: u32, e: Entry, q: vec2<f32>, lod: f32) -> vec4<f32> {
    let size = vec2<f32>(e.place.w, e.place.z);
    let origin = e.place.xy - e.more.xy * size;
    let drop = vec2<f32>(0.0, DROP * size.y);
    let soft = max(1.0, 0.05 * size.y);
    var body: vec4<f32>;
    var shade: f32;
    if (kind == SHAPE) {
        body = image_at(e, (q - origin) / size, lod);
        shade = image_at(e, (q - drop - origin) / size, min(lod + 1.5, MAX_LOD)).a;
    } else if (kind == DOT) {
        let r = 0.5 * size.y;
        let d = length(q - origin - vec2<f32>(r)) - r;
        let fill = clamp(0.5 - d, 0.0, 1.0);
        let white = clamp(d + DOT_LINE + 0.5, 0.0, 1.0);
        body = vec4<f32>(vec3<f32>(white) * fill, fill);
        shade = 1.0 - smoothstep(-soft, soft, length(q - drop - origin - vec2<f32>(r)) - r);
    } else {
        // Farther than a pixel past the arrow's own box, the body draws nothing; farther than the softness and a half
        // past it, nor does the shadow: neither polygon is evaluated there.
        let local = (q - origin) / size.y;
        body = vec4<f32>(0.0);
        if (near_arrow(local, 1.0 / size.y)) {
            let d = arrow(local) * size.y;
            let line = max(1.0, 0.07 * size.y);
            let fill = clamp(0.5 - d, 0.0, 1.0);
            let white = clamp(-d - line + 0.5, 0.0, 1.0);
            body = vec4<f32>(vec3<f32>(white) * fill, fill);
        }
        let low = (q - drop - origin) / size.y;
        shade = 0.0;
        if (near_arrow(low, (soft + 0.5) / size.y)) {
            shade = 1.0 - smoothstep(-soft, soft, arrow(low) * size.y);
        }
    }
    return body + (1.0 - body.a) * vec4<f32>(0.0, 0.0, 0.0, SHADOW * shade);
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let e = ring[in.index];
    let p = in.position.xy;
    let cover = kept(p, ring[u32(e.head.y)]);
    let kind = u32(e.head.x);
    if (kind == RIPPLE) {
        let d = abs(length(p - e.place.xy) - e.place.z) - 0.5 * RING * e.place.z;
        let a = clamp(0.5 - d, 0.0, 1.0) * e.head.z * cover;
        return vec4<f32>(e.image.rgb * a, a);
    }
    let smear = e.more.zw;
    let reach = length(smear);
    let n = select(0u, u32(clamp(reach * 0.5, 1.0, 16.0)), reach >= 1.0);
    // Only the taps landing in the look's box draw anything: the rest add exactly nothing, so they are skipped.
    let taps = tap_range(p, smear, e, n);
    var sum = vec4<f32>(0.0);
    for (var i = taps.x; i < taps.y; i++) {
        let t = select(0.0, f32(i) / f32(max(n, 1u)) - 0.5, n > 0u);
        sum += look(kind, e, p - smear * t, in.lod);
    }
    return sum / f32(n + 1u) * e.head.z * cover;
}

// Where along one axis a tap `p - s·t` stays inside `[lo, hi]`: the interval of t (empty when lo > hi).
fn slab(p: f32, s: f32, lo: f32, hi: f32) -> vec2<f32> {
    if (abs(s) < 1e-4) {
        return select(vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), p >= lo && p <= hi);
    }
    let a = (p - hi) / s;
    let b = (p - lo) / s;
    return vec2<f32>(min(a, b), max(a, b));
}

// The taps `[first, end)` of `n + 1` (t = i / n - 0.5) whose point lands in the look's box: the quad before the
// smear (`vs_main`'s), a pixel wider, outside which `look` is exactly zero (the image and its shadow are bounded
// by it, the Dot's and Arrow's coverage ends half a pixel out and their shadow `soft` out, both inside the margin).
fn tap_range(p: vec2<f32>, s: vec2<f32>, e: Entry, n: u32) -> vec2<u32> {
    if (n == 0u) {
        return vec2<u32>(0u, 1u);
    }
    let size = vec2<f32>(e.place.w, e.place.z);
    let origin = e.place.xy - e.more.xy * size;
    let margin = vec2<f32>(3.0 + 0.1 * size.y);
    let lo = origin - margin;
    let hi = origin + size + vec2<f32>(0.0, DROP * size.y) + margin;
    let x = slab(p.x, s.x, lo.x, hi.x);
    let y = slab(p.y, s.y, lo.y, hi.y);
    let nf = f32(n);
    let first = max(ceil((max(max(x.x, y.x), -0.5) + 0.5) * nf), 0.0);
    let last = min(floor((min(min(x.y, y.y), 0.5) + 0.5) * nf), nf);
    if (first > last) {
        return vec2<u32>(0u, 0u);
    }
    return vec2<u32>(u32(first), u32(last) + 1u);
}
