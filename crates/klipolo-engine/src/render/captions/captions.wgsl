// The caption renderer's shader. The first half builds a page's word atlas: `pack` packs the glyph
// passes' coverage into one texture, and `blur_pass` (run twice, across then down) blurs it for the shadow and glow,
// at half size for wide blurs (`halve` first, `combine` after). The second half draws a frame's captions
// from it: one instanced quad per shadow, outline, box and fill (`quad`, `shade`), in that order, over the frame. The
// atlas and its sampler are the first half's `src_a` and `src_s`; the frame's size and line boxes are `view` (binding
// 3).

@group(0) @binding(0) var src_a: texture_2d<f32>;
@group(0) @binding(1) var src_b: texture_2d<f32>;
@group(0) @binding(2) var src_s: sampler;

struct Full { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn full(@builtin(vertex_index) i: u32) -> Full {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Full(vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0), vec2<f32>(p.x, 1.0 - p.y));
}

// A pass's blurs, worked out on the host (`blur.rs`): its direction, each blur's pair count and centre weight (x the
// shadow's, y the glow's), whether the style glows, and the pairs, two a row (offset in texels, weight): the shadow's
// in rows 0–11, the glow's in rows 12–23. No transcendental maths per tap.
struct Blur {
    dir: vec2<f32>,
    pairs: vec2<u32>,
    centre: vec2<f32>,
    glow: f32,
    pad: f32,
    taps: array<vec4<f32>, 24>,
};
@group(1) @binding(0) var<uniform> blur: Blur;

// R: the fill; G: the outlined silhouette (it holds the fill too); B: the silhouette (a hard shadow, or the blur's
// source); A: the fill when the style glows (an unblurred glow, or the blur's source), else 0.
@fragment fn pack(v: Full) -> @location(0) vec4<f32> {
    let a = textureSample(src_a, src_s, v.uv).a;
    let b = textureSample(src_b, src_s, v.uv).a;
    let g = max(a, b);
    return vec4<f32>(a, g, g, a * blur.glow);
}

// Pair `i` of the blur whose pairs start at row `row`: (offset, weight).
fn pair(row: u32, i: u32) -> vec2<f32> {
    let t = blur.taps[row + i / 2u];
    return select(t.zw, t.xy, (i & 1u) == 0u);
}

// One direction of the two blurs: B ← blur(G) for the shadow, A ← blur(R) for the glow; R and G pass through. Each
// pair is one linear fetch each side, at its texels' weighted middle (as the built-in blur.wgsl reads).
@fragment fn blur_pass(v: Full) -> @location(0) vec4<f32> {
    let step = blur.dir / vec2<f32>(textureDimensions(src_a));
    let c = textureSampleLevel(src_a, src_s, v.uv, 0.0);
    // The first pass blurs G and R; the second reads its own B and A.
    let second = blur.dir.y > 0.5;
    var shadow = select(c.g, c.b, second) * blur.centre.x;
    for (var i = 0u; i < blur.pairs.x; i = i + 1u) {
        let t = pair(0u, i);
        let a = textureSampleLevel(src_a, src_s, v.uv + step * t.x, 0.0);
        let b = textureSampleLevel(src_a, src_s, v.uv - step * t.x, 0.0);
        shadow += (select(a.g, a.b, second) + select(b.g, b.b, second)) * t.y;
    }
    var glow = select(c.r, c.a, second) * blur.centre.y;
    for (var i = 0u; i < blur.pairs.y; i = i + 1u) {
        let t = pair(12u, i);
        let a = textureSampleLevel(src_a, src_s, v.uv + step * t.x, 0.0);
        let b = textureSampleLevel(src_a, src_s, v.uv - step * t.x, 0.0);
        glow += (select(a.r, a.a, second) + select(b.r, b.a, second)) * t.y;
    }
    return vec4<f32>(c.r, c.g, shadow, glow * blur.glow);
}

// A wide blur's source at half size: each texel the 2 × 2 average of the atlas texels it covers, one linear fetch at
// their shared corner.
@fragment fn halve(v: Full) -> @location(0) vec4<f32> {
    let s = textureSampleLevel(src_a, src_s, v.pos.xy * 2.0 / vec2<f32>(textureDimensions(src_a)), 0.0);
    return vec4<f32>(s.r, s.g, 0.0, 0.0);
}

// The half-size blurs upscaled into the atlas (linear; its texel k holds the atlas's 2k and 2k + 1). The pipeline
// writes only the masks that were blurred.
@fragment fn combine(v: Full) -> @location(0) vec4<f32> {
    let s = textureSampleLevel(src_a, src_s, v.pos.xy * 0.5 / vec2<f32>(textureDimensions(src_a)), 0.0);
    return vec4<f32>(0.0, 0.0, s.b, s.a);
}

// ---- The draw half ----

struct Inst {
    // Target px: centre x, centre y, width, height (before the turn).
    @location(0) rect: vec4<f32>,
    // The atlas cell (u0, v0, u1, v1).
    @location(1) uv: vec4<f32>,
    // The paint (straight RGB, alpha), and a second one (a fill's glow; a box's own size in px in zw).
    @location(2) color: vec4<f32>,
    @location(3) color2: vec4<f32>,
    // Turn in radians, kind (0 shadow, 1 outline, 2 box, 3 fill and glow), opacity, unused.
    @location(4) params: vec4<f32>,
    // Unused, a box's corner radius px, the line boxes' smoothing px, how many line boxes (0: one box).
    @location(5) extra: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) color2: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) extra: vec4<f32>,
};

// The output's size, and up to five line boxes (centre x, centre y, width, height px about the block's centre, scaled
// with the block) for a style that boxes lines: their smooth union is one shape (autofit's extra line included).
struct View { size: vec2<f32>, pad: vec2<f32>, lines: array<vec4<f32>, 5> };
@group(0) @binding(3) var<uniform> view: View;

// A 4-vertex strip per instance.
@vertex fn quad(@builtin(vertex_index) v: u32, i: Inst) -> VOut {
    let corner = vec2<f32>(f32(v & 1u), f32((v >> 1u) & 1u)) * 2.0 - 1.0;
    let c = cos(i.params.x);
    let s = sin(i.params.x);
    let local = corner * i.rect.zw * 0.5;
    let p = i.rect.xy + vec2<f32>(c * local.x - s * local.y, s * local.x + c * local.y);
    let ndc = vec2<f32>(p.x / view.size.x * 2.0 - 1.0, 1.0 - p.y / view.size.y * 2.0);
    let uv = mix(i.uv.xy, i.uv.zw, corner * 0.5 + 0.5);
    return VOut(vec4<f32>(ndc, 0.0, 1.0), uv, local, i.color, i.color2, i.params, i.extra);
}

fn sd_round_rect(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / max(k, 1e-3), 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

// Premultiplied output; the pipeline blends One, OneMinusSrcAlpha.
@fragment fn shade(v: VOut) -> @location(0) vec4<f32> {
    let kind = i32(v.params.y + 0.5);
    let alpha = v.params.z;
    if (kind == 2) {
        // Boxes: one rounded rect (color2.zw its size), or the smooth union of the line boxes.
        let r = v.extra.y;
        var d = sd_round_rect(v.local, v.color2.zw * 0.5, r);
        let lines = i32(v.extra.w + 0.5);
        if (lines > 0) {
            d = 1e9;
            for (var k = 0; k < lines; k = k + 1) {
                let l = view.lines[k];
                d = smin(d, sd_round_rect(v.local - l.xy, l.zw * 0.5, r), v.extra.z);
            }
        }
        let a = clamp(0.5 - d, 0.0, 1.0) * v.color.a * alpha;
        return vec4<f32>(v.color.rgb * a, a);
    }
    // Level 0 (the atlas has no mipmaps): no derivatives, so sampling after the box branch is fine.
    let m = textureSampleLevel(src_a, src_s, v.uv, 0.0);
    if (kind == 0) {
        let a = m.b * v.color.a * alpha;
        return vec4<f32>(v.color.rgb * a, a);
    }
    if (kind == 1) {
        let a = m.g * v.color.a * alpha;
        return vec4<f32>(v.color.rgb * a, a);
    }
    // The fill, then its glow added over it.
    let fill = m.r * v.color.a * alpha;
    let glow = m.a * v.color2.a * alpha;
    return vec4<f32>(v.color.rgb * fill + v.color2.rgb * glow, min(fill + glow, 1.0));
}
