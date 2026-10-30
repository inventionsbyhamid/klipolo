// Dust and bokeh: soft specks of light drifting slowly upward over `background`, in three layers of depth: the
// nearest large, faint and out of focus, the farthest small and sharp. `amount` is the share of places that hold a
// speck, `size` the nearest specks' radius in px, `blur` how far out of focus they are, `speed` how fast they rise.
// Each layer is a hashed grid of one speck a cell, kept inside its cell, so a pixel looks at one cell a layer. Mixed
// in linear light; smooth, so it draws at half size.

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let pix = uv * kl.size;
    var glow = 0.0;
    for (var i = 0; i < 3; i++) {
        let depth = f32(i);
        let scale = 1.0 / (1.0 + 0.9 * depth);
        let radius = max(params.size * kl.px * scale, 1.0);
        // Cells at least five radii wide (wider further back), so a speck kept in the middle of its cell stays in it.
        let cell = radius * (5.0 + 3.0 * depth);
        // Nearer layers rise faster (parallax), and each layer sways a little.
        let rise = kl.time * params.speed * 36.0 * kl.px * scale;
        let sway = sin(kl.time * 0.4 * params.speed + depth * 2.1) * cell * 0.25;
        let q = (pix + vec2<f32>(sway, rise)) / cell;
        let c = floor(q);
        let layer = kl.seed ^ (0xcb1ab31fu * u32(i + 1));
        let h = kl_hash((u32(i32(c.x)) * 0x8da6b343u) ^ (u32(i32(c.y)) * 0xd8163841u) ^ layer);
        let here = select(0.0, 1.0, f32(h & 0xffu) < params.amount * 255.0);
        // Anywhere in the middle 60 % of the cell: a speck's radius is at most a fifth of it, so it stays inside.
        let centre = vec2<f32>(0.2) + 0.6 * vec2<f32>(f32((h >> 8u) & 0xffu), f32((h >> 16u) & 0xffu)) / 255.0;
        let d = length(q - c - centre) * cell;
        // Out of focus: the edge softens over a share of the radius that grows with nearness, and over at least a
        // pixel, so the smallest specks stay round.
        let soft = max(radius * mix(0.15, 0.95, clamp(params.blur, 0.0, 1.0) * (1.0 - 0.45 * depth)), 1.0);
        let disc = 1.0 - smoothstep(radius - soft, radius, d);
        let phase = f32(h >> 24u) / 255.0 * 6.2831853;
        let twinkle = 0.65 + 0.35 * sin(kl.time * (0.8 + 0.6 * scale) + phase);
        // Nearer specks spread their light, so they are fainter.
        glow += here * disc * twinkle * (0.5 + 0.2 * depth);
    }
    let speck = clamp(glow, 0.0, 1.0) * params.color.a;
    let bg = params.background;
    let alpha = speck + bg.a * (1.0 - speck);
    let lit = kl_to_linear(params.color.rgb) * speck + kl_to_linear(bg.rgb) * (bg.a * (1.0 - speck));
    let lin = lit / max(alpha, 0.0001);
    return vec4<f32>(clamp(kl_from_linear(lin), vec3<f32>(0.0), vec3<f32>(1.0)) * alpha, alpha);
}
