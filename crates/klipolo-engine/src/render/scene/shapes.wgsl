// A scene's shapes (joined after `sdf.wgsl` and `paint.wgsl`, whose distances, gradients and clips it
// uses): one instanced pipeline drawing rects, ellipses, stars, polygons, lines and images as signed distance fields,
// each with its fill (a colour, one of the scene's gradients, or an image's texture), stroke,
// clips and opacity, and the analytic drop shadow of a rect, an ellipse or an image (its own instance, drawn before the
// shape and kept outside it, as CSS's box-shadow is). Each shape is held by up to two clips, which intersect: its own
// and its nearest clipped ancestor's. Output is premultiplied and gamma-encoded, like every effect
// target. Node space is centred on the node's box, in target pixels before the node's transform; `xf` maps it to target
// pixels.

// An image node's picture (a 1 × 1 stand-in for every other run).
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var picture_sampler: sampler;

struct Instance {
    // The 2 × 2 part of node space → target pixels (a, b, c, d: x' = a x + c y, y' = b x + d y).
    @location(0) xf: vec4<f32>,
    // The translation, then the box's half size (an image's: its picture's, contained in its box).
    @location(1) at_half: vec4<f32>,
    // kind (0 rect, 1 ellipse, 2 star or polygon, 3 line, 4 shadow, 5 image), points (a shadow's sigma), inner ratio (a
    // shadow's corner), how far the quad reaches past the half size.
    @location(2) shape: vec4<f32>,
    // Corner radii as `sd_round_rect` reads them: top right, bottom right, top left, bottom left (a shadow's: its
    // shape's, which it stays outside of).
    @location(3) radius: vec4<f32>,
    // Straight RGBA: the fill (a gradient's tint), the stroke (a shadow's colour). A shadow's `fill` is its shape's
    // centre and half size in the shadow's space.
    @location(4) fill: vec4<f32>,
    @location(5) stroke: vec4<f32>,
    // Fill gradient (−1 none, −2 a generator fill's target, −3 a backdrop), stroke gradient (−1 none), the fill
    // gradient's turn (radians) and slide (a backdrop's: its map's translation).
    @location(6) paint: vec4<f32>,
    // Stroke width, opacity, line cap (0 butt, 1 round, 2 square; a shadow's shape: 0 rect, 1 ellipse), the stroke
    // gradient's turn (a shadow's: its shape's flat alpha, the least its fill can be).
    @location(7) misc: vec4<f32>,
    // Two clips, each in its owner's node space: target pixels → that space (the 2 × 2 part, then its translation, the
    // clip's kind (0 none, 1 inset, 2 circle) and an inset's round), then an inset's left, top, right and bottom edges,
    // or a circle's centre and radius.
    @location(8) clip_m: vec4<f32>,
    @location(9) clip_t: vec4<f32>,
    @location(10) clip: vec4<f32>,
    @location(11) clip2_m: vec4<f32>,
    @location(12) clip2_t: vec4<f32>,
    @location(13) clip2: vec4<f32>,
    // The stroke gradient's slide, 0, 0, 0 (a backdrop's: its map, target pixels → the copy's uv, a b c d).
    @location(14) extra: vec4<f32>,
};

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) local_half: vec4<f32>,
    @location(1) @interpolate(flat) shape: vec4<f32>,
    @location(2) @interpolate(flat) radius: vec4<f32>,
    @location(3) @interpolate(flat) fill: vec4<f32>,
    @location(4) @interpolate(flat) stroke: vec4<f32>,
    @location(5) @interpolate(flat) paint: vec4<f32>,
    @location(6) @interpolate(flat) misc: vec4<f32>,
    @location(7) @interpolate(flat) clip_m: vec4<f32>,
    @location(8) @interpolate(flat) clip_t: vec4<f32>,
    @location(9) @interpolate(flat) clip: vec4<f32>,
    @location(10) @interpolate(flat) clip2_m: vec4<f32>,
    @location(11) @interpolate(flat) clip2_t: vec4<f32>,
    @location(12) @interpolate(flat) clip2: vec4<f32>,
    @location(13) @interpolate(flat) extra: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> Varyings {
    // Two triangles over the node's box, grown by the reach.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let half = inst.at_half.zw;
    let local = corners[vi] * (half + vec2<f32>(inst.shape.w));
    let p = vec2<f32>(
        inst.xf.x * local.x + inst.xf.z * local.y,
        inst.xf.y * local.x + inst.xf.w * local.y,
    ) + inst.at_half.xy;
    var out: Varyings;
    out.position = clip_space(p);
    // The half size rides with the position: interpolating a constant keeps it exact.
    out.local_half = vec4<f32>(local, half);
    out.shape = inst.shape;
    out.radius = inst.radius;
    out.fill = inst.fill;
    out.stroke = inst.stroke;
    out.paint = inst.paint;
    out.misc = inst.misc;
    out.clip_m = inst.clip_m;
    out.clip_t = inst.clip_t;
    out.clip = inst.clip;
    out.clip2_m = inst.clip2_m;
    out.clip2_t = inst.clip2_t;
    out.clip2 = inst.clip2;
    out.extra = inst.extra;
    return out;
}

// The shape's distance in node space: a shadow's is its shape's (it stays outside it), an image's its rounded box.
fn shape_distance(
    p: vec2<f32>,
    half: vec2<f32>,
    shape: vec4<f32>,
    radius: vec4<f32>,
    misc: vec4<f32>,
    fill: vec4<f32>,
) -> f32 {
    let kind = shape.x;
    if kind < 0.5 {
        return sd_round_rect(p, half, radius);
    }
    if kind < 1.5 {
        return sd_ellipse(p, half);
    }
    if kind < 2.5 {
        return sd_star(p, min(half.x, half.y), shape.y, shape.z);
    }
    if kind < 3.5 {
        return sd_line(p, half, misc.x, misc.z);
    }
    if kind < 4.5 {
        if misc.z > 0.5 {
            return sd_ellipse(p - fill.xy, fill.zw);
        }
        return sd_round_rect(p - fill.xy, fill.zw, radius);
    }
    return sd_round_rect(p, half, radius);
}

@fragment
fn fs_main(v: Varyings) -> @location(0) vec4<f32> {
    let p = v.local_half.xy;
    let half = v.local_half.zw;
    // Derivatives first, in uniform control flow. Two clips intersect: the farther outside of the two.
    let at = scene_px(v.position);
    let dc = max(
        clip_distance(at, v.clip_m, v.clip_t, v.clip),
        clip_distance(at, v.clip2_m, v.clip2_t, v.clip2),
    );
    let d = shape_distance(p, half, v.shape, v.radius, v.misc, v.fill);
    let aa_clip = max(fwidth(dc), 1e-4);
    let aa = max(fwidth(d), 1e-4);
    let uv = p / max(2.0 * half, vec2<f32>(1e-3)) + vec2<f32>(0.5);
    let uv_dx = dpdx(uv);
    let uv_dy = dpdy(uv);
    let opacity = v.misc.y * coverage(dc, aa_clip);
    let kind = v.shape.x;
    if kind > 3.5 && kind < 4.5 {
        // A shadow: its rect's blurred coverage in its colour (the stroke slot), kept outside its shape.
        let corner = min(v.shape.z, min(half.x, half.y));
        // Kept to the part of the pixel outside its shape, for any alpha: the shape drawn over it lets
        // `1 − A` through (A: its flat alpha `misc.w` × opacity × coverage), so the shadow is weighted
        // `(1 − coverage) / (1 − A)`. An opaque shape's rim keeps its whole shadow (no light seam); a glass card, a
        // fade inside the scene or an unfilled outline shows no dark band inside its edge.
        let cf = coverage(d, aa);
        let shape_alpha = v.misc.w * opacity * cf;
        let outside = select(clamp((1.0 - cf) / max(1.0 - shape_alpha, 1e-4), 0.0, 1.0), 0.0, cf >= 1.0);
        return premul(v.stroke) * rounded_shadow(p, half, v.shape.y, corner) * outside * opacity;
    }
    let size = 2.0 * half;
    var stroke = premul(v.stroke);
    if v.paint.y >= 0.0 {
        stroke = gradient(i32(v.paint.y), uv, size, v.misc.w, v.extra.x) * premul(v.stroke);
    }
    if kind > 2.5 && kind < 3.5 {
        return stroke * coverage(d, aa) * opacity;
    }
    var fill = premul(v.fill);
    if kind > 4.5 || (v.paint.x < -1.5 && v.paint.x > -2.5) {
        // An image's picture (premultiplied, as decoded), sampled by its own mip level; or a generator fill's target
        // across the box.
        fill = textureSampleGrad(picture, picture_sampler, uv, uv_dx, uv_dy) * v.fill.a;
    } else if v.paint.x < -2.5 {
        // A backdrop: the frame under the layer (its blurred copy), from this target's pixel, under the tint
        // (the stroke slot: a backdrop has no stroke).
        let buv = vec2<f32>(dot(v.extra.xy, at), dot(v.extra.zw, at)) + v.paint.zw;
        let tint = premul(v.stroke);
        fill = tint + textureSampleLevel(picture, picture_sampler, buv, 0.0) * (1.0 - tint.a);
    } else if v.paint.x >= 0.0 {
        fill = gradient(i32(v.paint.x), uv, size, v.paint.z, v.paint.w) * premul(v.fill);
    }
    let cf = coverage(d, aa);
    var cs = 0.0;
    if v.misc.x > 0.0 {
        cs = coverage(abs(d) - v.misc.x * 0.5, aa);
    }
    let sc = stroke * cs;
    return (sc + fill * cf * (1.0 - sc.a)) * opacity;
}
