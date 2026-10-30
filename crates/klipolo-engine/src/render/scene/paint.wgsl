// What a scene's shapes and paths share (moved out of `shapes.wgsl`, joined after `sdf.wgsl`): the
// target's size and place, the scene's gradient table (group 0, one bind group for both pipelines), a gradient's colour at a point
// of its box, and a clip's distance.

struct Globals {
    // The target's size in pixels, and its top left in the scene's target (an offscreen node's target is only its own
    // box). Node places and clips are in the scene target's pixels.
    size: vec2<f32>,
    origin: vec2<f32>,
};

struct Gradient {
    // kind (1 linear, 2 radial), stop count, 0, 0.
    head: vec4<f32>,
    // Linear: x0, y0, x1, y1; radial: cx, cy, r, 0 (fractions of the node's box).
    geom: vec4<f32>,
    // Sixteen stop offsets, four to a vector.
    pos: array<vec4<f32>, 4>,
    // Premultiplied colours.
    color: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

// The scene target's pixel a fragment of this target is at.
fn scene_px(position: vec4<f32>) -> vec2<f32> {
    return position.xy + globals.origin;
}

// A scene target pixel as clip space in this target.
fn clip_space(at: vec2<f32>) -> vec4<f32> {
    let p = at - globals.origin;
    return vec4<f32>(p.x / globals.size.x * 2.0 - 1.0, 1.0 - p.y / globals.size.y * 2.0, 0.0, 1.0);
}
@group(0) @binding(1) var<uniform> gradients: array<Gradient, 32>;

fn stop_at(gi: i32, i: i32) -> f32 {
    return gradients[gi].pos[i / 4][i % 4];
}

// Gradient `gi` at `uv` (fractions of a box of `size` pixels), turned by `turn` about the box's centre (in pixels, so
// the angle on screen is the one written, whatever the box's shape) and slid by `slide`: premultiplied, mixed between
// its stops (held at its ends, as SVG pads).
fn gradient(gi: i32, uv: vec2<f32>, size: vec2<f32>, turn: f32, slide: f32) -> vec4<f32> {
    let head = gradients[gi].head;
    let geom = gradients[gi].geom;
    let c = cos(-turn);
    let s = sin(-turn);
    let box_size = max(size, vec2<f32>(1e-3));
    let d = (uv - vec2<f32>(0.5)) * box_size;
    let q = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y) / box_size + vec2<f32>(0.5);
    var t = 0.0;
    if head.x < 1.5 {
        let ab = geom.zw - geom.xy;
        t = dot(q - geom.xy, ab) / max(dot(ab, ab), 1e-6);
    } else {
        t = length(q - geom.xy) / max(geom.z, 1e-6);
    }
    t = clamp(t - slide, 0.0, 1.0);
    let n = i32(head.y);
    var out = gradients[gi].color[0];
    for (var i = 1; i < 16; i = i + 1) {
        if i >= n {
            break;
        }
        let p0 = stop_at(gi, i - 1);
        let p1 = stop_at(gi, i);
        if t >= p1 {
            out = gradients[gi].color[i];
        } else if t > p0 {
            out = mix(gradients[gi].color[i - 1], gradients[gi].color[i], (t - p0) / max(p1 - p0, 1e-6));
        }
    }
    return out;
}

// The clip's distance at scene target pixel `at`, in its owner's node space; far inside without one, far outside when it is
// closed (an inset whose edges met, a circle of radius 0: nothing shows, not even an antialiased hairline).
fn clip_distance(at: vec2<f32>, m: vec4<f32>, t: vec4<f32>, clip: vec4<f32>) -> f32 {
    if t.z < 0.5 {
        return -1e4;
    }
    let p = vec2<f32>(m.x * at.x + m.z * at.y, m.y * at.x + m.w * at.y) + t.xy;
    if t.z < 1.5 {
        let centre = (clip.xy + clip.zw) * 0.5;
        let half = (clip.zw - clip.xy) * 0.5;
        if half.x <= 0.0 || half.y <= 0.0 {
            return 1e4;
        }
        return sd_round_rect(p - centre, half, vec4<f32>(min(t.w, min(half.x, half.y))));
    }
    if clip.z <= 0.0 {
        return 1e4;
    }
    return length(p - clip.xy) - clip.z;
}
