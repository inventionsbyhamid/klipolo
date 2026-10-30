// A scene's paths (joined after `sdf.wgsl` and `paint.wgsl`): a path's fill or stroke, tessellated once,
// drawn with its paint, its trim and its dashes. A stroke's vertices sit on its centre line and move out by the draw's
// half width (so one mesh serves every width), a fill's edge vertices out by half a pixel each way (its antialiased
// fringe, the inside drawn a half pixel in). Trim and dashes are cut by distance along the path, each cut shaped by the
// line's cap; a stroke is antialiased across by its distance from the centre line; a translucent stroke is drawn
// through the stencil so it blends once a pixel where it overlaps itself. Output is premultiplied, as the shapes'.

struct PathDraw {
    // Mesh units → target pixels: the 2 × 2 part (a, b, c, d), then the translation, how far `dir` reaches (a stroke's
    // half width and a pixel's fringe; a fill's half pixel), and 1 for a stroke, 0 for a fill.
    m: vec4<f32>,
    t: vec4<f32>,
    // Mesh units → the paint's box (fractions): scale x, y, offset x, y.
    uv: vec4<f32>,
    // The paint's box in pixels, the stroke's half width in mesh units, and a thin stroke's alpha (drawn half a pixel
    // wide at most this strong).
    box_hw: vec4<f32>,
    // Straight RGBA (a gradient's tint).
    color: vec4<f32>,
    // The gradient (−1 none), its turn (radians) and slide, the opacity.
    paint: vec4<f32>,
    // The trim: where it starts and how long it is (mesh units, wrapping), the path's length, and flags: 1 its start
    // is the path's own (an open path's end), 2 its end is, 4 no trim.
    trim: vec4<f32>,
    // Up to eight dash and gap lengths (mesh units); their count, period and offset, and the line's cap (0 butt, 1
    // round, 2 square), which every trim and dash cut takes.
    dash0: vec4<f32>,
    dash1: vec4<f32>,
    dash: vec4<f32>,
    // Two clips, as the shapes' (`clip_distance`).
    clip_m: vec4<f32>,
    clip_t: vec4<f32>,
    clip: vec4<f32>,
    clip2_m: vec4<f32>,
    clip2_t: vec4<f32>,
    clip2: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> draws: array<PathDraw>;

struct PathVertex {
    @location(0) pos: vec2<f32>,
    @location(1) dir: vec2<f32>,
    @location(2) side: f32,
    @location(3) along: f32,
    @location(4) sub: f32,
};

struct PathVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // A stroke's side (−1 to 1 across it); a fill's coverage (1 inside, 0 at its fringe's outer edge).
    @location(1) across: f32,
    // A stroke's distance along the path, and along its sub-path (where its dashes start again).
    @location(2) along: f32,
    @location(3) sub: f32,
    @location(4) @interpolate(flat) draw: u32,
};

@vertex
fn vs_path(v: PathVertex, @builtin(instance_index) i: u32) -> PathVaryings {
    let d = draws[i];
    let stroke = d.t.w > 0.5;
    let reach = select(v.side * d.t.z, d.t.z, stroke);
    let p = v.pos + v.dir * reach;
    let at = vec2<f32>(d.m.x * p.x + d.m.z * p.y, d.m.y * p.x + d.m.w * p.y) + d.t.xy;
    var out: PathVaryings;
    out.position = clip_space(at);
    out.uv = p * d.uv.xy + d.uv.zw;
    out.across = select(v.along, v.side, stroke);
    out.along = v.along;
    out.sub = v.along - v.sub;
    out.draw = i;
    return out;
}

// How much of a pixel at distance `sd` inside an edge (negative outside) is covered, `aa` the pixel's size.
fn edge(sd: f32, aa: f32) -> f32 {
    return clamp(sd / aa + 0.5, 0.0, 1.0);
}

// The trim at `x` along a path of length `total`: how far inside `[start, start + len]` (wrapping) `x` is, along the
// path (negative past a cut; far inside without a trim, or where an end is the path's own).
fn trimmed(t: vec4<f32>, x_in: f32) -> f32 {
    let flags = u32(t.w + 0.5);
    let far = 1e9;
    if (flags & 4u) != 0u {
        return far;
    }
    let total = max(t.z, 1e-6);
    let x = clamp(x_in, 0.0, total);
    let rel = select(x - t.x, x - t.x + total, x < t.x);
    let to_start = select(rel, far, (flags & 1u) != 0u);
    let to_end = select(t.y - rel, far, (flags & 2u) != 0u);
    let back = select(total - rel, far, (flags & 1u) != 0u);
    let past = select(rel - t.y, far, (flags & 2u) != 0u);
    return select(-min(past, back), min(to_start, to_end), rel <= t.y);
}

fn dash_len(d: PathDraw, k: u32) -> f32 {
    if k < 4u {
        return d.dash0[k];
    }
    return d.dash1[k - 4u];
}

// The dashes at `x` along a sub-path: how far inside a dash `x` is (negative in a gap, by the distance to the nearer
// dash end; far inside without dashes). A dash that starts with the sub-path has no cut there: that end is its cap.
fn dashed(d: PathDraw, x: f32) -> f32 {
    let n = u32(d.dash.x + 0.5);
    let period = d.dash.y;
    if n == 0u || period <= 0.0 {
        return 1e9;
    }
    let p = x + d.dash.z;
    let m = p - period * floor(p / period);
    var start = 0.0;
    var sd = 0.0;
    for (var k = 0u; k < 8u; k = k + 1u) {
        if k >= n {
            break;
        }
        let end = start + dash_len(d, k);
        if m <= end || k + 1u == n {
            let on = k % 2u == 0u;
            let back = select(m - start, 1e9, on && x - (m - start) <= 1e-4 * period);
            let inside = min(back, end - m);
            sd = select(-inside, inside, on);
            break;
        }
        start = end;
    }
    return sd;
}

// What a fragment of a draw shows: its premultiplied paint, its coverage, and its opacity with the clips'.
struct Shade {
    paint: vec4<f32>,
    cover: f32,
    opacity: f32,
};

fn shade(v: PathVaryings) -> Shade {
    let d = draws[v.draw];
    // Derivatives first, in uniform control flow.
    let at = scene_px(v.position);
    let dc = max(
        clip_distance(at, d.clip_m, d.clip_t, d.clip),
        clip_distance(at, d.clip2_m, d.clip2_t, d.clip2),
    );
    let aa_clip = max(fwidth(dc), 1e-4);
    let w_across = fwidth(v.across);
    let aa_along = max(fwidth(v.along), 1e-6);
    var cover = clamp(v.across, 0.0, 1.0);
    if d.t.w > 0.5 {
        // A stroke: its distance from the centre line against its half width, across; its trim and dashes along,
        // each cut taking the line's cap: round, a disc of the half width past the cut; square, the
        // half width further; butt, at the cut.
        let reach = d.t.z;
        let hw = d.box_hw.z;
        let aa = max(w_across * reach, 1e-6);
        let across = abs(v.across) * reach;
        let sd = min(trimmed(d.trim, v.along), dashed(d, v.sub));
        let cap = u32(d.dash.w + 0.5);
        if cap == 1u {
            cover = edge(hw - length(vec2<f32>(max(-sd, 0.0), across)), aa);
        } else {
            let ext = select(0.0, hw, cap == 2u);
            cover = edge(hw - across, aa) * edge(sd + ext, aa_along);
        }
        cover = cover * d.box_hw.w;
    }
    var paint = premul(d.color);
    if d.paint.x >= 0.0 {
        paint = gradient(i32(d.paint.x), v.uv, d.box_hw.xy, d.paint.y, d.paint.z) * premul(d.color);
    }
    return Shade(paint, cover, d.paint.w * coverage(dc, aa_clip));
}

@fragment
fn fs_path(v: PathVaryings) -> @location(0) vec4<f32> {
    let s = shade(v);
    return s.paint * s.cover * s.opacity;
}

// A translucent stroke drawn once a pixel: first the pixels it covers whole, each stencilled with
// the draw's own reference so nothing of the same stroke blends there again...
@fragment
fn fs_path_solid(v: PathVaryings) -> @location(0) vec4<f32> {
    let s = shade(v);
    if s.cover < 0.998 {
        discard;
    }
    return s.paint * s.opacity;
}

// ...then its antialiased edge, only where the stroke hasn't drawn yet, once.
@fragment
fn fs_path_edge(v: PathVaryings) -> @location(0) vec4<f32> {
    let s = shade(v);
    if s.cover <= 0.002 {
        discard;
    }
    return s.paint * s.cover * s.opacity;
}
