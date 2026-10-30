// Signed distances and coverage for a scene's shapes (`shapes.wgsl` is joined after this): rounded rects
// (`sd_round_rect`), ellipses, stars and polygons, lines with caps, and a rounded rect's Gaussian shadow
// (Evan Wallace's closed form along x, four samples along y). Distances are in node pixels.

fn sd_round_rect(p: vec2<f32>, half: vec2<f32>, r: vec4<f32>) -> f32 {
    // r: top-right, bottom-right, top-left, bottom-left (y down: the left pair is picked first, then top or bottom).
    var rr = select(r.zw, r.xy, p.x > 0.0);
    rr.x = select(rr.y, rr.x, p.y < 0.0);
    let q = abs(p) - half + vec2<f32>(rr.x);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rr.x;
}

// An ellipse's distance, approximated: exact on the curve and near it, which is all coverage needs.
fn sd_ellipse(p: vec2<f32>, r: vec2<f32>) -> f32 {
    let rs = max(r, vec2<f32>(1e-3));
    let k0 = length(p / rs);
    let k1 = max(length(p / (rs * rs)), 1e-6);
    return k0 * (k0 - 1.0) / k1;
}

// A star of `n` points (outer radius `big`, inner `big × inner`), its first point up; a regular polygon is a star whose
// inner points sit on its edges (`inner = cos(π / n)`). The point is folded into one half wedge, where the edge is the
// segment from an outer point to the next inner one.
fn sd_star(p: vec2<f32>, big: f32, n: f32, inner: f32) -> f32 {
    let wedge = 3.14159265 / max(n, 3.0);
    // The angle from straight up, clockwise (y down), folded into [0, wedge].
    let a0 = atan2(p.x, -p.y);
    let m = a0 - 2.0 * wedge * floor(a0 / (2.0 * wedge));
    let a = abs(m - wedge);
    let a_fold = wedge - a;
    let q = length(p) * vec2<f32>(cos(a_fold), sin(a_fold));
    let outer = vec2<f32>(big, 0.0);
    let inn = big * clamp(inner, 0.01, 1.0) * vec2<f32>(cos(wedge), sin(wedge));
    let e = inn - outer;
    let w = q - outer;
    let h = clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
    let dist = length(w - e * h);
    // Inside when on the centre's side of the edge.
    let side = e.x * w.y - e.y * w.x;
    let centre = e.x * (-outer.y) - e.y * (-outer.x);
    return select(dist, -dist, side * centre >= 0.0);
}

// A line from the box's top left to its bottom right, `width` wide, with its caps.
fn sd_line(p: vec2<f32>, half: vec2<f32>, width: f32, cap: f32) -> f32 {
    let a = -half;
    let ab = 2.0 * half;
    let len = max(length(ab), 1e-3);
    let dir = ab / len;
    let w = p - a;
    let u = dot(w, dir);
    let v = abs(dir.x * w.y - dir.y * w.x);
    if cap > 0.5 && cap < 1.5 {
        let h = clamp(u, 0.0, len);
        return length(w - dir * h) - width * 0.5;
    }
    let ext = select(0.0, width * 0.5, cap > 1.5);
    return max(v - width * 0.5, max(-u - ext, u - len - ext));
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    // Abramowitz and Stegun 7.1.27: within 5e-4 of erf.
    let s = sign(x);
    let a = abs(x);
    let y = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    let y2 = y * y;
    return s - s / (y2 * y2);
}

// The blur of a rounded rect's coverage along x at height y, closed form (a Gaussian's integral is erf), where the
// corners' round makes the rect narrower.
fn shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (0.70710678 / sigma));
    return integral.y - integral.x;
}

// A rounded rect's Gaussian shadow at `p` (node space, the shadow's own box): x in closed form, y by four samples.
fn rounded_shadow(p: vec2<f32>, half: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    let s = max(sigma, 0.25);
    let low = p.y - half.y;
    let high = p.y + half.y;
    let start = clamp(-3.0 * s, low, high);
    let end = clamp(3.0 * s, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var value = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        let g = exp(-(y * y) / (2.0 * s * s)) / (sqrt(6.2831853) * s);
        value = value + shadow_x(p.x, p.y - y, s, corner, half) * g * step;
        y = y + step;
    }
    return value;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn coverage(d: f32, aa: f32) -> f32 {
    return clamp(0.5 - d / aa, 0.0, 1.0);
}
