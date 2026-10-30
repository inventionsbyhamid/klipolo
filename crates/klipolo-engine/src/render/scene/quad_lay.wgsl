// An offscreen node laid back as a quad (joined after `group_clip.wgsl`): a normal-blend node that
// neither tilts nor masks is drawn over the target it began in as one quad over its own target's box, blended
// premultiplied (source over, by the pipeline), with its opacity and its clips (its own and the two nearest around it,
// in the scene target's pixels), antialiased as the host pass's are (`composite.wgsl`). What it reads is premultiplied
// and gamma-encoded, as every target of the scene is, and transparent outside what the node draws.

// The node's slot (`slots.rs`): its target's size and place in the scene's (unread here), its box in the target it is
// laid into (x, y, w, h), that target's size and top left in the scene's, its opacity, and its three clips.
struct Slot {
    size: vec2<f32>,
    origin: vec2<f32>,
    boxed: vec4<f32>,
    parent: vec4<f32>,
    misc: vec4<f32>,
    c0m: vec4<f32>,
    c0t: vec4<f32>,
    c0c: vec4<f32>,
    c1m: vec4<f32>,
    c1t: vec4<f32>,
    c1c: vec4<f32>,
    c2m: vec4<f32>,
    c2t: vec4<f32>,
    c2c: vec4<f32>,
};

@group(0) @binding(0) var<uniform> lay: Slot;
@group(0) @binding(1) var lay_sampler: sampler;
// The node's target, as the effect stage binds its targets (the second texture unused).
@group(1) @binding(0) var node: texture_2d<f32>;

@vertex
fn vs_lay(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    let q = lay.boxed.xy + corners[v % 4u] * lay.boxed.zw;
    return vec4<f32>(q.x / lay.parent.x * 2.0 - 1.0, 1.0 - q.y / lay.parent.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_lay(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // The clips first, their derivative in uniform control flow: the farthest outside of the three.
    let at = position.xy + lay.parent.zw;
    let dc = max(
        group_clip(at, lay.c0m, lay.c0t, lay.c0c),
        max(group_clip(at, lay.c1m, lay.c1t, lay.c1c), group_clip(at, lay.c2m, lay.c2t, lay.c2c)),
    );
    let clipped = clamp(0.5 - dc / max(fwidth(dc), 1e-4), 0.0, 1.0);
    let uv = (position.xy - lay.boxed.xy) / max(lay.boxed.zw, vec2<f32>(1.0));
    return textureSampleLevel(node, lay_sampler, uv, 0.0) * lay.misc.x * clipped;
}
