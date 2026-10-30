//! The effect stage's side of the main app's guards: checked shaders compiled off the drawing
//! thread once their compile mark is on disk, drawn once their draw mark is (`handshake`); the preview's
//! registry (its pipelines kept, its built-ins and then its checked shaders compiled as soon as they are placed); the
//! sampler's steps; and the notes and events they make.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use klipolo_model::contrib::ContribId;

use super::EffectStage;
use super::compile_thread::PipelineCompiler;
use super::handshake::{GuardEvent, Purpose};
use super::sampling::{Degrade, Step};
use crate::registry::{Registry, ShaderEffect, ShaderTrust};
use crate::render::mode::RenderMode;

/// A shader preset (`registry::presets`): a built-in whose compile can wait for the first agent draft.
pub(super) fn is_preset(shader: &ShaderEffect) -> bool {
    shader.trust == ShaderTrust::Builtin
        && shader.id.is_builtin()
        && shader
            .id
            .as_str()
            .split_once('/')
            .is_some_and(|(_, local)| crate::registry::presets::body(local).is_some())
}

impl EffectStage {
    pub fn mode(&self) -> RenderMode {
        self.mode
    }

    /// The signal the compile thread fires when a pipeline is ready (the compositor's redraw: the still is drawn again).
    pub fn set_wake(&mut self, wake: Arc<tokio::sync::Notify>) {
        if let Some(compiler) = &self.compiler {
            compiler.set_wake(wake.clone());
        }
        self.wake = Some(wake);
    }

    /// An export's stage: checked passes predicted over 200 ms are drawn in tiles (`tiles`).
    pub(crate) fn set_tiled(&mut self, on: bool) {
        self.tiled = on;
    }

    /// Asks for `shader`'s pipeline (`Interactive`), once: the built-ins are queued on the compile thread first,
    /// then its compile mark is asked for, and the compile thread gets it only once that mark is on disk
    /// `mark_written`), so it runs after every built-in.
    pub(super) fn ask(&mut self, shader: &ShaderEffect) {
        if self.asked.contains_key(&shader.key)
            || self.awaiting.iter().any(|(_, s)| s.key == shader.key)
        {
            return;
        }
        self.queue_builtins();
        let seq = self.handshake.ask_compile(shader);
        self.awaiting.push((seq, Arc::new(shader.clone())));
    }

    /// At the stage's first checked request: every built-in not made yet goes to the compile thread
    /// ahead of any agent compile (its queue is first in, first out). The drawing thread compiles none at placement,
    /// and never waits on the scope lock behind an agent's compile: a built-in a frame needs before it is handed back
    /// compiles there behind at most another built-in. The shader presets aren't among them:
    /// they go behind the first agent compile (`queue_presets`), so eight compiles never delay an agent's first draft; a
    /// frame that needs one sooner compiles it itself, as any built-in's first use outside the preview does.
    fn queue_builtins(&mut self) {
        if self.builtins_asked {
            return;
        }
        self.builtins_asked = true;
        self.queue_where(|s| !is_preset(s));
    }

    /// Built-in pipelines a motion graphic's host passes need (its lay-back, mask and tint), to the compile thread
    /// (`Interactive` only) unless made or queued already: the lookahead asks as a
    /// scene comes up, so its first frame never compiles them on the drawing thread when no warm-up ran. Elsewhere a
    /// frame compiles them at first use, as before.
    pub(crate) fn queue_also(&mut self, shaders: &[&Arc<ShaderEffect>]) {
        if self.mode != crate::render::mode::RenderMode::Interactive {
            return;
        }
        for shader in shaders {
            if !self.pipelines.contains(shader) && !self.builtins_queued.contains_key(&shader.key) {
                self.compiler().request((*shader).clone());
                self.builtins_queued.insert(shader.key, (*shader).clone());
            }
        }
    }

    /// The shader presets not made yet, to the compile thread behind the first checked compile.
    fn queue_presets(&mut self) {
        if self.presets_asked {
            return;
        }
        self.presets_asked = true;
        self.queue_where(is_preset);
    }

    /// The built-ins `pick` takes that aren't made or queued yet, to the compile thread.
    fn queue_where(&mut self, pick: impl Fn(&ShaderEffect) -> bool) {
        for shader in self.builtin_shaders() {
            if pick(shader.as_ref())
                && !self.pipelines.contains(&shader)
                && !self.builtins_queued.contains_key(&shader.key)
            {
                self.compiler().request(shader.clone());
                self.builtins_queued.insert(shader.key, shader);
            }
        }
    }

    /// Every built-in pipeline the stage may draw with: the stage's own, and the registry's built-ins with their second
    /// passes (`warm`'s list).
    fn builtin_shaders(&self) -> Vec<Arc<ShaderEffect>> {
        let mut all = vec![
            self.grab.clone(),
            self.resample.clone(),
            self.overlay.clone(),
            super::padding::pad_shader().clone(),
        ];
        for shader in Registry::builtin().shaders() {
            all.push(shader.clone());
            all.extend(shader.then.clone());
        }
        all
    }

    /// The compile thread, made with its first request.
    fn compiler(&mut self) -> &PipelineCompiler {
        self.compiler.get_or_insert_with(|| {
            let compiler = PipelineCompiler::new(self.dev.clone(), self.pipeline_layout.clone());
            if let Some(wake) = &self.wake {
                compiler.set_wake(wake.clone());
            }
            compiler
        })
    }

    /// `shader` to the compile thread under its compile mark `seq` (on disk); the shader presets go behind the first.
    fn send_compile(&mut self, shader: Arc<ShaderEffect>, seq: u64) {
        self.compiler().request(shader.clone());
        self.asked.insert(shader.key, (shader, seq));
        self.queue_presets();
    }

    /// The pipelines the compile thread handed back since the last call. A built-in is kept unless the drawing thread
    /// made it meanwhile (one that failed compiles there, as before). A checked one closes its compile mark and is kept
    /// unless the preview's registry let its shader go meanwhile (an effect drawn with it then asks for its own draw
    /// mark when a frame needs it); one that couldn't be compiled is left out, said once. Never waits.
    pub(crate) fn poll_compiles(&mut self) {
        let Some(compiler) = &self.compiler else {
            return;
        };
        for (key, made) in compiler.take_ready() {
            if let Some(builtin) = self.builtins_queued.remove(&key) {
                if let Ok(pipeline) = made
                    && !self.pipelines.contains(&builtin)
                {
                    self.pipelines.insert(&builtin, Some(pipeline), self.frame);
                }
                continue;
            }
            let Some((shader, seq)) = self.asked.remove(&key) else {
                continue;
            };
            self.handshake.compile_returned(key, seq);
            if self.pins.as_ref().is_some_and(|pins| !pins.contains(&key)) {
                continue;
            }
            match made {
                Ok(pipeline) => self.pipelines.insert(&shader, Some(pipeline), self.frame),
                Err(_) => {
                    self.pipelines.insert(&shader, None, self.frame);
                    self.notes.push(format!(
                        "The {} effect couldn't be drawn, so it's left out.",
                        shader.name
                    ));
                }
            }
        }
    }

    /// Whether `shader` (a built-in) waits on the compile thread (`ensure_pipeline` polls for it first).
    pub(super) fn builtin_queued(&self, shader: &ShaderEffect) -> bool {
        self.builtins_queued.contains_key(&shader.key)
    }

    /// The app wrote the crash-guard mark `seq` of `id` to disk (`EngineCommand::MarkWritten`, m1): a compile mark sends
    /// its shader to the compile thread; a draw mark lets it draw from the next frame on.
    pub fn mark_written(&mut self, id: &ContribId, seq: u64) {
        if self.handshake.ack(id, seq, Instant::now()) == Some(Purpose::Compile)
            && let Some(i) = self.awaiting.iter().position(|(s, _)| *s == seq)
        {
            let (_, shader) = self.awaiting.remove(i);
            self.send_compile(shader, seq);
        }
    }

    /// Closes the draw marks whose time ran out: 2 s after the GPU finished their first drawing frame, or after an
    /// acknowledgement that never drew.
    pub fn guard_tick(&mut self) {
        let now = Instant::now();
        let (done_frame, done_at) = self.gpu_done.newest();
        self.handshake
            .tick_done(now, done_frame, done_at.unwrap_or(now));
        self.gpu_done.set_waiting(self.handshake.waiting_for_gpu());
    }

    /// Where the poll thread writes the newest frame the GPU finished (the backend's callback).
    pub fn gpu_done(&self) -> Arc<super::handshake::GpuDone> {
        self.gpu_done.clone()
    }

    /// This stage's frame number (the frame just drawn, after a render).
    pub fn frame_number(&self) -> u64 {
        self.frame
    }

    /// When the next open draw mark runs out of time.
    pub fn guard_deadline(&self) -> Option<Instant> {
        self.handshake.deadline()
    }

    /// The registry the preview draws with from now on (the project feed's): only its shaders' pipelines stay, and none
    /// of them is ever evicted (bounded by the project, not by the cap of 32); an open mark let go
    /// closes, except a compile's in flight, kept until it returns. In `Interactive` mode its checked shaders
    /// are asked for now, when they are placed rather than when the playhead reaches them, behind every built-in on the
    /// compile thread (with the half-size step, `RESAMPLE`); no draw mark is asked for until a frame needs one.
    ///.
    pub fn prepare(&mut self, registry: &Registry) {
        let own = [
            self.grab.clone(),
            self.resample.clone(),
            self.overlay.clone(),
            super::padding::pad_shader().clone(),
        ];
        let mut keep: Vec<&ShaderEffect> = own.iter().map(|s| &**s).collect();
        for shader in registry.shaders() {
            keep.push(shader);
            keep.extend(shader.then.as_deref());
        }
        self.pipelines.retain(&keep);
        let keys: HashSet<u64> = keep.iter().map(|s| s.key).collect();
        let checked: Vec<&ShaderEffect> = keep
            .iter()
            .copied()
            .filter(|s| s.trust == ShaderTrust::Checked)
            .collect();
        let placed = |k: u64, id: &ContribId| checked.iter().any(|s| s.key == k && s.id == *id);
        // A compile in flight keeps its mark until it returns.
        let in_flight = &self.asked;
        self.handshake
            .let_go(|k, id, compile| (compile && in_flight.contains_key(&k)) || placed(k, id));
        self.awaiting.retain(|(_, s)| placed(s.key, &s.id));
        self.pipelines.set_pins(keys.clone());
        self.pins = Some(keys);
        if self.mode != RenderMode::Interactive || checked.is_empty() {
            return;
        }
        for shader in checked {
            if !self.pipelines.contains(shader) {
                self.ask(shader);
            }
        }
    }

    /// After the frame's passes: what the sampler timed is resolved into its read buffer (`PassSampler::end_frame`), and
    /// whether a draw mark now waits for the GPU to finish this frame.
    pub fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.pass_sampler.end_frame(encoder);
        self.gpu_done.set_waiting(self.handshake.waiting_for_gpu());
    }

    /// The notes this stage has for the frame being drawn: checked shaders that couldn't be compiled, effects slowed down.
    pub fn frame_notes(&mut self, warnings: &mut Vec<String>) {
        warnings.append(&mut self.notes);
    }

    /// What the app hears about agent-made effects since the last call (`GuardEvent`).
    pub fn take_guard_events(&mut self) -> Vec<GuardEvent> {
        std::mem::take(&mut self.handshake.events)
    }

    /// The device was lost at `lost_at`: the checked effects to strike (open acknowledged marks, every draw whose frame
    /// the GPU never finished, and those drawn in the last 60 frames and 2 s before the loss); unacknowledged marks close
    /// close. The stage's marks go.
    pub fn lost_marks(&mut self, lost_at: Instant) -> Vec<ContribId> {
        let done = self.gpu_done.newest().0;
        self.handshake.lost(self.frame, done, lost_at)
    }

    /// Whether a draw mark waits for the GPU to finish its first drawing frame (only then does the backend ask to hear
    /// of it).
    pub fn waiting_for_gpu(&self) -> bool {
        self.handshake.waiting_for_gpu()
    }

    /// The project closed (the engine reset): every effect the sampler slowed down draws in full again.
    pub fn project_closed(&mut self) {
        self.pass_sampler.clear();
        self.slow_said.clear();
    }

    /// The sampler slowed `step.id` down: said once, as an event, and noted on the frame once per effect.
    pub(super) fn slowed(&mut self, step: Step) {
        if !self.slow_said.contains(&step.id) {
            self.slow_said.push(step.id.clone());
            self.notes
                .push(crate::api::agent_effect_slow(step.id.as_str()));
        }
        self.handshake.events.push(GuardEvent::Degraded {
            contrib: step.id,
            to: step.to,
            ms: step.ms,
            budget_ms: step.budget_ms,
        });
    }

    /// Whether the preview's sampler slowed `shader` (a generator's own pass) down to half size.
    pub(crate) fn halved(&self, shader: &ShaderEffect) -> bool {
        self.degraded(shader) == Some(Degrade::Half)
    }

    /// How the sampler has `shader` drawn (built-ins: always in full).
    pub(super) fn degraded(&self, shader: &ShaderEffect) -> Option<Degrade> {
        if shader.trust != ShaderTrust::Checked {
            return None;
        }
        self.pass_sampler.level(&shader.id)
    }

    /// Requests the compile thread hasn't handed back yet (tests).
    #[doc(hidden)]
    pub fn pipelines_pending(&self) -> usize {
        self.compiler.as_ref().map_or(0, |c| c.pending())
    }

    /// Pipelines compiled on the drawing thread so far (tests).
    #[doc(hidden)]
    pub fn drawing_thread_compiles(&self) -> u64 {
        self.sync_compiles
    }

    /// Crash-guard marks this stage holds open (tests).
    #[doc(hidden)]
    pub fn open_marks(&self) -> usize {
        self.handshake.open()
    }

    /// Whether every built-in pipeline (the registry's built-ins and the stage's own) is made (tests).
    #[doc(hidden)]
    pub fn builtins_ready(&self) -> bool {
        self.builtin_shaders()
            .iter()
            .all(|s| self.pipelines.contains(s))
    }

    /// Whether the half-size step's pipeline is made (tests).
    #[doc(hidden)]
    pub fn half_size_step_ready(&self) -> bool {
        self.pipelines.contains(&self.resample)
    }

    /// Checked pipelines kept for drawing (tests).
    #[doc(hidden)]
    pub fn checked_pipelines_kept(&self) -> usize {
        self.pipelines.checked_len()
    }

    /// The submissions the checked passes of the last frame took (tests).
    #[doc(hidden)]
    pub fn checked_submissions(&self) -> u32 {
        self.checked_submissions
    }

    /// The sampler's test hooks (`PassSampler::set_clock`, `set_on`, `cost`).
    #[doc(hidden)]
    pub fn sampler_mut(&mut self) -> &mut super::sampling::PassSampler {
        &mut self.pass_sampler
    }

    #[doc(hidden)]
    pub fn sampler(&self) -> &super::sampling::PassSampler {
        &self.pass_sampler
    }
}
