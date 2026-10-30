// Waves: open water seen from a little above, swells rolling in before one wind, the distance catching the sky and a
// path of sun glitter. `density` sets how many swells there are and `speed` how fast they roll; `deepColor` is the
// water's own colour, seen close and from above, and `shallowColor` (labelled Sky colour) the sky it reflects, which
// grows towards the distance (Fresnel); `foam` is how much foam the highest crests carry. The water is
// five swells within about 12° of the wind, on alternating sides (wider
// spreads cross into a plaid lattice), each 1.8 times finer and half as tall, with narrow crests and broad troughs,
// each swell's slope worked out with its height (no extra evaluations for the normal), and the finer swells fading
// where they would be finer than the pixels. Lit in linear light. No texture reads, and smooth, so it draws at half
// size.

// One swell along `dir` (a unit vector), `amp` tall: its height (narrow crests, broad troughs) and that height's
// slope. With s = 0.5 + 0.5·sin(x), the height is 2s² − 1 and its slope 2s·cos(x) times `freq` along `dir`.
fn swell(p: vec2<f32>, dir: vec2<f32>, freq: f32, phase: f32, amp: f32) -> vec3<f32> {
    let x = dot(p, dir) * freq + phase;
    let s = 0.5 + 0.5 * sin(x);
    return amp * vec3<f32>(2.0 * s * s - 1.0, (2.0 * s * cos(x) * freq) * dir);
}

// A swell `freq` times as fine as the first, kept while a wave of it spans at least 4 pixels (`blur` is how much water
// 4 pixels cover here) and faded out as it gets finer.
fn kept(freq: f32, blur: f32) -> f32 {
    return clamp(6.2832 / max(freq * blur, 0.0001) - 0.5, 0.0, 1.0);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    // The camera looks out over the water from a little above: rows further up are further away. `p` is a point on
    // the water (across, and away from the camera), `view` the way back to the camera.
    let aspect = kl.size.x / max(kl.size.y, 1.0);
    let dist = 1.0 / (uv.y + 0.6);
    let across = (uv.x - 0.5) * aspect * dist;
    let p = vec2<f32>(across, -dist) * 6.0;
    let view = normalize(vec3<f32>(-across, dist, 0.5));
    let blur = 24.0 * dist * dist / max(kl.size.y, 1.0);
    // Finer swells roll a little faster, each from its own phase.
    let t = kl.time * params.speed;
    let f = params.density;
    var w = swell(p, vec2<f32>(0.1219, -0.9925), f, t, 0.5);
    w += swell(p, vec2<f32>(-0.1564, -0.9877), f * 1.8, t * 1.35 + 1.7, 0.25);
    w += swell(p, vec2<f32>(0.0698, -0.9976), f * 3.24, t * 1.8 + 4.1, 0.125 * kept(f * 3.24, blur));
    w += swell(p, vec2<f32>(-0.2079, -0.9781), f * 5.83, t * 2.4 + 2.3, 0.0625 * kept(f * 5.83, blur));
    w += swell(p, vec2<f32>(0.1736, -0.9848), f * 10.5, t * 3.2 + 5.9, 0.03125 * kept(f * 10.5, blur));
    let h = w.x;
    let n = normalize(vec3<f32>(-w.y, -w.z, 4.0));
    // How much sky the water reflects (Schlick's Fresnel), and the sun's glitter: the sun is low, ahead and to the
    // right, across in step with the frame's width, so its path stays about two thirds of the way across in every
    // format.
    let facing = clamp(dot(n, view), 0.0, 1.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - facing, 5.0);
    let sun = normalize(vec3<f32>(0.157 * aspect, -0.9315, 0.2329));
    let glint = pow(max(dot(n, normalize(view + sun)), 0.0), 300.0);
    let water = kl_to_linear(params.deepColor.rgb) * (0.8 + 0.4 * clamp(0.5 + h, 0.0, 1.0));
    let sky = kl_to_linear(params.shallowColor.rgb) * 1.4;
    var c = mix(water, sky, clamp(fresnel * 1.8, 0.0, 1.0)) + vec3<f32>(1.2 * glint);
    c = mix(c, vec3<f32>(0.8), smoothstep(0.7, 0.9, h) * params.foam);
    return vec4<f32>(clamp(kl_from_linear(c), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
