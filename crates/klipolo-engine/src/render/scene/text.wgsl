// A scene's text (joined after `sdf.wgsl` and `paint.wgsl`, whose distances, gradients and clips it
// shares): one instanced quad per piece of text (a grapheme, a word or a line, as its node is cut), moved, scaled and
// turned with its split unit, in its colour (or the node's gradient, attached to the letters where they rest) with its
// outline under it, both from the piece's cell of the instance's atlas (R the fill, G the silhouette; a colour glyph's
// premultiplied pixels in the colour plane); and one rounded rect per span background, drawn under its pieces. Each
// quad is held by up to three clips, which intersect: its box when its text fits (`fit`), its node's own clip, and its
// nearest clipped ancestor's. A piece that blurs (its unit's or its node's `blur`, at most 12 units a frame)
// grows by its blur and reads its cell blurred from the scratch target (`text_blur.wgsl`, drawn small);
// its colour glyphs stay sharp. A span's gradient spans its own letters. What a node's pieces of one look share is in
// the frame's paints, which each piece names; a node whose shadow is baked draws it under its
// piece, in the piece's own quad, from its cell of the shadows' texture, and fades with it. Output is premultiplied, as
// the shapes'.

struct TextDraw {
    // Box pixels → target pixels: the 2 × 2 part (a, b, c, d), then the translation, the kind (0 a piece, 1 a span's
    // background) and a background's corner radius in box pixels.
    m: vec4<f32>,
    t: vec4<f32>,
    // The quad in box pixels (left, top, right, bottom) where the letters rest, and its cell in the atlas (0–1).
    rect: vec4<f32>,
    uv: vec4<f32>,
    // Its opacity, its paint (index), how far a blurring piece's quad grows (its Gaussian in box pixels; 0 sharp),
    // and that Gaussian in atlas texels (above 0: it reads its blur back).
    misc: vec4<f32>,
    // A blurring piece's blur in the scratch target, in its pixels: blur_a.xy + atlas uv × blur_a.zw, inside blur_b.
    blur_a: vec4<f32>,
    blur_b: vec4<f32>,
};

struct TextPaint {
    // Straight RGBA: the fill (a gradient's tint), and the outline.
    fill: vec4<f32>,
    outline: vec4<f32>,
    // The gradient (−1 none), its turn (radians) and slide, and 1 when its colour glyphs draw.
    paint: vec4<f32>,
    // The gradient's box: its size and its top left, in box pixels.
    gbox: vec4<f32>,
    // Three clips, as the shapes' (`clip_distance`): the box a fitted text keeps to, the node's own, its ancestor's.
    c0_m: vec4<f32>,
    c0_t: vec4<f32>,
    c0: vec4<f32>,
    c1_m: vec4<f32>,
    c1_t: vec4<f32>,
    c1: vec4<f32>,
    c2_m: vec4<f32>,
    c2_t: vec4<f32>,
    c2: vec4<f32>,
    // A baked shadow: its colour (straight; alpha 0, none), its offset in box pixels, its cell's box where it rests
    // (box pixels) and its uv in the shadows' texture.
    shadow: vec4<f32>,
    shadow_at: vec4<f32>,
    shadow_rect: vec4<f32>,
    shadow_uv: vec4<f32>,
    // Where the colour plane lies in the atlas: its top left in atlas texels, and one over its size.
    cmap: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> texts: array<TextDraw>;
@group(1) @binding(1) var masks: texture_2d<f32>;
@group(1) @binding(2) var colors: texture_2d<f32>;
@group(1) @binding(3) var atlas_sampler: sampler;
// The frame's blurring pieces blurred (`text_blur.wgsl`; a transparent texel when nothing blurs).
@group(1) @binding(4) var blurred: texture_2d<f32>;
@group(1) @binding(5) var<storage, read> paints: array<TextPaint>;
// The baked shadows (a transparent texel without any).
@group(1) @binding(6) var shadows: texture_2d<f32>;

struct TextVaryings {
    @builtin(position) position: vec4<f32>,
    // The atlas, and the quad's point in box pixels where the letters rest.
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
    @location(2) @interpolate(flat) draw: u32,
    @location(3) @interpolate(flat) paint: u32,
};

@vertex
fn vs_text(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> TextVaryings {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[v % 6u];
    let d = texts[i];
    let pi = u32(d.misc.y);
    let p = paints[pi];
    // A background grows by a pixel each way, so its own edge antialiases inside the quad; a blurred piece by its blur;
    // a piece with a baked shadow over its shadow, where it falls.
    let blur = max(d.misc.z, 0.0);
    let grow = select(0.0, 1.5, d.t.z > 0.5) + select(0.0, 3.0 * blur + 1.0, blur > 0.0);
    var r = d.rect + vec4<f32>(-grow, -grow, grow, grow);
    if p.shadow.w > 0.0 && d.t.z < 0.5 {
        let s = p.shadow_rect + p.shadow_at.xyxy;
        r = vec4<f32>(min(r.xy, s.xy), max(r.zw, s.zw));
    }
    let local = mix(r.xy, r.zw, c);
    let at = vec2<f32>(d.m.x * local.x + d.m.z * local.y, d.m.y * local.x + d.m.w * local.y) + d.t.xy;
    var out: TextVaryings;
    out.position = clip_space(at);
    // The atlas where the quad's point rests: the cell's uv, carried on past it as far as the quad grew.
    let span = max(d.rect.zw - d.rect.xy, vec2<f32>(1e-3));
    out.uv = d.uv.xy + (local - d.rect.xy) / span * (d.uv.zw - d.uv.xy);
    out.local = local;
    out.draw = i;
    out.paint = pi;
    return out;
}

@fragment
fn fs_text(v: TextVaryings) -> @location(0) vec4<f32> {
    let d = texts[v.draw];
    let p = paints[v.paint];
    // Derivatives first, in uniform control flow. The clips intersect: the farthest outside of the three. A baked
    // shadow keeps to its node's own and its ancestor's, not to the box a fitted text keeps its letters to.
    let at = scene_px(v.position);
    let d_box = clip_distance(at, p.c0_m, p.c0_t, p.c0);
    let d_up = max(clip_distance(at, p.c1_m, p.c1_t, p.c1), clip_distance(at, p.c2_m, p.c2_t, p.c2));
    let dc = max(d_box, d_up);
    let aa_clip = max(fwidth(dc), 1e-4);
    let aa_box = max(fwidth(d_box), 1e-4);
    let aa_up = max(fwidth(d_up), 1e-4);
    let half = (d.rect.zw - d.rect.xy) * 0.5;
    let sd = sd_round_rect(v.local - (d.rect.xy + half), half, vec4<f32>(min(d.t.w, min(half.x, half.y))));
    let aa = max(fwidth(sd), 1e-4);
    let opacity = d.misc.x * coverage(dc, aa_clip);
    var fill = premul(p.fill);
    if p.paint.x >= 0.0 {
        let box_size = max(p.gbox.xy, vec2<f32>(1.0));
        fill = gradient(i32(p.paint.x), (v.local - p.gbox.zw) / box_size, box_size, p.paint.y, p.paint.z) * premul(p.fill);
    }
    if d.t.z > 0.5 {
        return fill * coverage(sd, aa) * opacity;
    }
    // The atlas has no mipmaps: level 0, sampled within the piece's cell only (its quad may reach past it).
    let inside = all(v.local >= d.rect.xy) && all(v.local <= d.rect.zw);
    var m = select(vec4<f32>(0.0), textureSampleLevel(masks, atlas_sampler, v.uv, 0.0), inside);
    let own = select(vec4<f32>(0.0), colour_at(v.uv, p.cmap), inside) * p.paint.w;
    if d.misc.w > 0.0 {
        m = vec4<f32>(blurred_at(v.uv, d), 0.0, 0.0);
    }
    // The colour glyphs over the fill, then the outline under both, then the baked shadow under all of it, faded with
    // them as one.
    let top = own + fill * m.r * (1.0 - own.a);
    let line = premul(p.outline) * m.g;
    let shown = top + line * (1.0 - top.a);
    if p.shadow.w > 0.0 {
        let letters = shown * coverage(d_box, aa_box);
        let shadow = premul(p.shadow) * shadow_at(v.local, p);
        return (letters + shadow * (1.0 - letters.a)) * d.misc.x * coverage(d_up, aa_up);
    }
    return shown * opacity;
}

// The colour plane (lying at `cmap` in the atlas) at atlas `uv`: transparent outside it, as it covers only the colour
// glyphs.
fn colour_at(uv: vec2<f32>, cmap: vec4<f32>) -> vec4<f32> {
    let atlas = vec2<f32>(textureDimensions(masks));
    let q = (uv * atlas - cmap.xy) * cmap.zw;
    if any(q < vec2<f32>(0.0)) || any(q > vec2<f32>(1.0)) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(colors, atlas_sampler, q, 0.0);
}

// The baked shadow's coverage at box pixel `local` (its cell moved by its offset; nothing past its cell).
fn shadow_at(local: vec2<f32>, p: TextPaint) -> f32 {
    let q = local - p.shadow_at.xy;
    if any(q < p.shadow_rect.xy) || any(q > p.shadow_rect.zw) {
        return 0.0;
    }
    let span = max(p.shadow_rect.zw - p.shadow_rect.xy, vec2<f32>(1e-3));
    let uv = p.shadow_uv.xy + (q - p.shadow_rect.xy) / span * (p.shadow_uv.zw - p.shadow_uv.xy);
    return textureSampleLevel(shadows, atlas_sampler, uv, 0.0).r;
}

// A blurring piece's masks at atlas `uv`, read back from its region of the blur's scratch target (`text_blur.wgsl`:
// drawn small, blurred across and down): one bilinear fetch, kept within the region.
fn blurred_at(uv: vec2<f32>, d: TextDraw) -> vec2<f32> {
    let size = vec2<f32>(textureDimensions(blurred));
    let at = clamp(d.blur_a.xy + uv * d.blur_a.zw, d.blur_b.xy + vec2<f32>(0.5), d.blur_b.zw - vec2<f32>(0.5));
    return textureSampleLevel(blurred, atlas_sampler, at / size, 0.0).rg;
}
