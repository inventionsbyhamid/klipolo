//! The OS backends' pure helpers: no OS call, no `real-capture`, built on every OS, so the Linux CI runs test the
//! macOS and Windows rules too. Each OS's backend calls its own.
//! - `camera`: a camera's format and frame rate for a preset, what its notices mean, how a suspended one counts;
//! - `cursor_checks`: the macOS cursor source's self-checks: a monitor that misses buttons or moves, the event clock;
//! - `cursor_looks`: the macOS cursor's shape looks: no current cursor, the cursor seed, the shape limit, animations;
//! - `cursor_map`: the cursor's place in the captured area and its event times on the host clock;
//! - `cursor_pace`: the macOS cursor sampler's schedule and its shapes' sizes;
//! - `mouse_mask`: the macOS mouse monitor's event mask, mouse bits only;
//! - `mac_preflight`: Core Audio's transports, the `Info.plist` facts that mark a game, and the folders whose bundles
//!   macOS guards behind a prompt;
//! - `our_damage`: frames that changed only Klipolo's own windows (the camera bubble, the HUD), handled as idle;
//! - `probe_plan`: the Mac probe's schedule within its budget;
//! - `sck`: ScreenCaptureKit's pacing, stop codes, picture size, sound, cursor and still-screen rules;
//! - `shared_stream`: a screen's stream start as the sound that shares it waits on it;
//! - `sleep`: how long the computer slept, by the wall clock;
//! - `still`: the screen encoder thread's schedule: re-feeds, nudges, a pipe-sourced cursor's heartbeat;
//! - `vt_ids`: which VideoToolbox encoders run apart from the GPU, and on which engine.

pub mod camera;
pub mod cursor_checks;
pub mod cursor_looks;
pub mod cursor_map;
pub mod cursor_pace;
pub mod mac_preflight;
pub mod mouse_mask;
pub mod our_damage;
pub mod probe_plan;
pub mod sck;
pub mod shared_stream;
pub mod sleep;
pub mod still;
pub mod vt_ids;
