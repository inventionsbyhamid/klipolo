// An offscreen node's clips where it is laid back (shared by `composite.wgsl` and `quad_lay.wgsl`).

// The clip's distance at `q` (the scene target's pixels) in its owner's node space, as the scene's shapes read it
// (`paint.wgsl`): far inside without one, far outside when it is closed.
fn group_clip(q: vec2<f32>, m: vec4<f32>, t: vec4<f32>, clip: vec4<f32>) -> f32 {
    if t.z < 0.5 {
        return -1e4;
    }
    let p = vec2<f32>(m.x * q.x + m.z * q.y, m.y * q.x + m.w * q.y) + t.xy;
    if t.z < 1.5 {
        let centre = (clip.xy + clip.zw) * 0.5;
        let half = (clip.zw - clip.xy) * 0.5;
        if half.x <= 0.0 || half.y <= 0.0 {
            return 1e4;
        }
        let r = min(t.w, min(half.x, half.y));
        let d = abs(p - centre) - half + vec2<f32>(r);
        return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0))) - r;
    }
    if clip.z <= 0.0 {
        return 1e4;
    }
    return length(p - clip.xy) - clip.z;
}
