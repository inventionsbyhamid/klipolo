// Klipolo effect ABI v1.1: the host prelude, prepended to every effect. v1.1 adds to v1; v1 effects draw the same.
//
// An effect file defines one entry function, plus any helper functions and constants it needs. It declares no
// bindings and no entry points; the host adds those.
//   fn effect(uv: vec2<f32>) -> vec4<f32>      filters (read the layer with kl_sample_a) and generators (no input)
//   fn transition(uv: vec2<f32>) -> vec4<f32>  transitions: from kl_a (outgoing) to kl_b (incoming) by kl.progress
// `uv` runs 0–1 across the layer from its top left. Settings arrive in `params`, a struct the host generates from
// the contribution's schema, in schema order:
//   float, angle  -> f32 (angles in radians)    int -> i32    bool -> u32 (0 or 1)    choice -> u32 (option index)
//   point -> vec2<f32> (0–1)                     colour -> vec4<f32>, straight (not premultiplied) RGBA 0–1
// Sizes declared in px are 1080p pixels: multiply by kl.px for this pass's pixels.
// A filter that draws past its layer (padding) makes the pass's `uv` cover a bigger, grown target: the layer's own box
// is `kl.content`, and shapes, centres and grids are measured from it (kl_content_uv, kl_content_size).
//
// Colour: textures and the return value are premultiplied RGBA with BT.709 primaries, gamma-encoded with the
// sRGB/BT.709 display curve, like the frame itself. Use kl_to_linear and kl_from_linear around maths that needs
// linear light.
// Determinism: there is no wall clock and no state between frames. kl.time is timeline time, and kl_rand is seeded
// per layer and effect, so preview, scrubbing and export draw the same pixels.
// Generators: kl.time, kl.progress and kl.frame follow the layer's piece of the generator's time, so the second piece
// of a split layer continues the first; with `fps` they hold on 1/fps steps. A generator that reads none of them
// draws once and is reused while its settings and size stay the same (static).
// Cost: one full-screen pass at the layer's drawn size (a generator with `resolution: half`, at half of it). Untrusted
// shaders run with bounds and loop checks.

struct KlFrame {
    size: vec2<f32>,  // this pass's output in pixels
    time: f32,        // seconds since the layer started (timeline time; a generator layer's: its piece's time)
    progress: f32,    // 0–1 across the layer, or across the transition
    frame: u32,       // frame index on the 30 fps grain clock (a generator layer's: of its piece's time)
    seed: u32,        // stable per layer and effect (a split generator's pieces keep the whole layer's)
    px: f32,          // this pass's pixels per 1080p pixel
    _pad: f32,
    content: vec4<f32>, // the layer's own box in this pass's uv (x, y, w, h): (0, 0, 1, 1) unless padding grew it
    fps: f32,         // v1.1: the project's frame rate
    duration: f32,    // v1.1: seconds kl.progress runs across: the layer (a split generator's whole span) or transition
    resolution: f32,  // v1.1: this pass's resolution: 1, halved by each halving; kl.size and kl.px include it
    step_fps: f32,    // v1.1: the rate kl.time steps at (a generator's `fps`), 0 when it runs smoothly
};

struct KlVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var<uniform> kl: KlFrame;
@group(0) @binding(1) var kl_samp: sampler;
@group(1) @binding(0) var kl_a: texture_2d<f32>;
@group(1) @binding(1) var kl_b: texture_2d<f32>;

@vertex
fn kl_vs(@builtin(vertex_index) i: u32) -> KlVertex {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: KlVertex;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn kl_sample_a(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(kl_a, kl_samp, uv, 0.0);
}

fn kl_sample_b(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(kl_b, kl_samp, uv, 0.0);
}

// `uv` across the layer's own box (0–1 over it; outside it in a grown target's margin).
fn kl_content_uv(uv: vec2<f32>) -> vec2<f32> {
    return (uv - kl.content.xy) / kl.content.zw;
}

// The layer's own box in this pass's pixels.
fn kl_content_size() -> vec2<f32> {
    return kl.size * kl.content.zw;
}

// One pixel of this pass in uv units.
fn kl_texel() -> vec2<f32> {
    return 1.0 / kl.size;
}

fn kl_premultiply(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn kl_unpremultiply(c: vec4<f32>) -> vec4<f32> {
    if (c.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, c.a);
}

fn kl_to_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

fn kl_from_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

// BT.709 luma of gamma-encoded RGB.
fn kl_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn kl_ease_in(t: f32) -> f32 {
    let x = clamp(t, 0.0, 1.0);
    return x * x;
}

fn kl_ease_out(t: f32) -> f32 {
    let x = clamp(t, 0.0, 1.0);
    return 1.0 - (1.0 - x) * (1.0 - x);
}

fn kl_ease_in_out(t: f32) -> f32 {
    let x = clamp(t, 0.0, 1.0);
    return x * x * (3.0 - 2.0 * x);
}

fn kl_hash(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return h;
}

// 0–1, the same for a pixel of this layer on this frame in every render.
fn kl_rand(uv: vec2<f32>) -> f32 {
    let p = vec2<u32>(clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * kl.size);
    return f32(kl_hash(p.x ^ kl_hash(p.y ^ kl_hash(kl.seed ^ kl.frame)))) / 4294967296.0;
}
