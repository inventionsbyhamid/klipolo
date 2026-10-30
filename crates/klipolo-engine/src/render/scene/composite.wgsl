// A scene's offscreen groups put back (host passes of the effect stage, each one full pass at the size of
// the target the group began in, reading two of its targets; joined after `group_clip.wgsl`). `comp` lays `kl_b` (a
// group's target, only as big as its own box: `gbox`, in this pass's pixels) over `kl_a` (the target
// the group began in, whose top left in the scene's target is `porig`, or a shadow under the group) with the group's
// opacity, blend mode, clips and 2.5D tilt; `mask` keeps a group where another node is opaque or bright; `tint` makes a
// group's silhouette in its shadow's colour. Every texture is premultiplied, gamma-encoded and transparent where nothing
// draws, as the scene's own target is.

// W3C's blend modes on premultiplied colour, `b` over `a` (mode 0 normal, 1 multiply, 2 screen, 3 overlay, 4 add),
// exact over transparent pixels too (where nothing is below, the group shows as it is).
fn group_blend(mode: f32, a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    if mode < 0.5 {
        return b + a * (1.0 - b.a);
    }
    if mode > 3.5 {
        return vec4<f32>(min(b.rgb + a.rgb, vec3<f32>(1.0)), b.a + a.a * (1.0 - b.a));
    }
    let cb = select(vec3<f32>(0.0), a.rgb / max(a.a, 1e-6), a.a > 0.0);
    let cs = select(vec3<f32>(0.0), b.rgb / max(b.a, 1e-6), b.a > 0.0);
    var mixed = cs * cb;
    if mode > 1.5 && mode < 2.5 {
        mixed = cs + cb - cs * cb;
    } else if mode > 2.5 {
        mixed = mix(2.0 * cs * cb, vec3<f32>(1.0) - 2.0 * (vec3<f32>(1.0) - cs) * (vec3<f32>(1.0) - cb), step(vec3<f32>(0.5), cb));
    }
    let rgb = (1.0 - a.a) * b.rgb + (1.0 - b.a) * a.rgb + a.a * b.a * mixed;
    return vec4<f32>(rgb, b.a + a.a - a.a * b.a);
}

// Where target pixel `q` shows the tilted group from: the group's own pixel (xy) and whether it is in front of the
// viewer (z). The group's plane turns about `tilto.xy` by the rotation whose top 2 × 2 is `tiltr` and whose third row
// starts `tiltz.xy`, seen from `tilto.z` pixels away (CSS `perspective`): q − o = d·R₂u / (d − r₃·u), so
// u = d·(d·R₂ + (q − o)·r₃ᵀ)⁻¹·(q − o).
fn untilt(q: vec2<f32>) -> vec3<f32> {
    let o = params.tilto.xy;
    let d = params.tilto.z;
    let w = q - o;
    let m00 = d * params.tiltr.x + w.x * params.tiltz.x;
    let m01 = d * params.tiltr.y + w.x * params.tiltz.y;
    let m10 = d * params.tiltr.z + w.y * params.tiltz.x;
    let m11 = d * params.tiltr.w + w.y * params.tiltz.y;
    let det = m00 * m11 - m01 * m10;
    // No early return: the clips' derivatives are taken after this, in uniform control flow.
    let safe = select(det, 1.0, abs(det) < 1e-6);
    let u = vec2<f32>(m11 * w.x - m01 * w.y, m00 * w.y - m10 * w.x) * (d / safe);
    let z = params.tiltz.x * u.x + params.tiltz.y * u.y;
    let front = abs(det) >= 1e-6 && d - z > 1e-3;
    return select(vec3<f32>(0.0), vec3<f32>(o + u, 1.0), front);
}

fn inside01(uv: vec2<f32>) -> f32 {
    return select(0.0, 1.0, all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0)));
}

fn transition(uv: vec2<f32>) -> vec4<f32> {
    // This pass's pixel, and the scene target's (where the clips and the tilt are).
    let q = uv * kl.size;
    let at = q + params.porig.xy;
    // Where a tilted group's pixel comes from (its own clip is in its own, turned space: `tiltz.z`), then the clips,
    // their derivative in uniform control flow: the farthest outside of the three, antialiased.
    let tilted = params.tilto.w > 0.5;
    let p = untilt(at);
    let own = select(at, p.xy, tilted && params.tiltz.z > 0.5);
    let dc = max(
        group_clip(own, params.c0m, params.c0t, params.c0c),
        max(group_clip(at, params.c1m, params.c1t, params.c1c), group_clip(at, params.c2m, params.c2t, params.c2c)),
    );
    let clipped = clamp(0.5 - dc / max(fwidth(dc), 1e-4), 0.0, 1.0);
    // Below (the target the group began in, read texel for texel), or the shadow moved by its offset.
    let auv = uv - params.misc.zw / kl.size;
    let a = kl_sample_a(auv) * inside01(auv);
    // The group's own target: its box in this pass's pixels.
    let src = select(q, p.xy - params.porig.xy, tilted);
    let buv = (src - params.gbox.xy) / max(params.gbox.zw, vec2<f32>(1.0));
    let shown = select(1.0, p.z, tilted) * inside01(buv);
    let b = kl_sample_b(buv) * shown * params.misc.y * clipped;
    return group_blend(params.misc.x, a, b);
}
