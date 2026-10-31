import { create } from 'zustand';
import type { StateReason } from '../bindings/StateReason';
import { engine } from '../bridge/engine';
import { errorMessage } from '../lib/errors';
import { frameIndexAt, frameStartUs } from '../lib/time';
import { noteFrameShown } from './framesShown';
import { clockAnchor, clockUs, holdClock, reportFrame } from './playClock';
import { toast } from './toasts';

/** What a drawn frame tells playback (from the frame trailer, or the native presenter's `shown` event). */
export interface FrameMeta {
  timeUs: number;
  seq: number;
  playback: boolean;
  /** When it is on screen (performance time, ms); now when not given. */
  atMs?: number;
}

interface PlaybackStore {
  /** What is on screen: optimistic on seek, the drawn frame's time while playing. */
  playheadUs: number;
  /** Rust owns the clock; this mirrors it. */
  playing: boolean;
  /** The speed it plays at (J/K/L shuttle: −2, −1, 1 or 2; negative backward); 1 when stopped. */
  rate: number;
  /** Sequence of the latest seek; frames rendered before the engine applied it are ignored. */
  seq: number;
  seek: (us: number) => void;
  /** Plays from the playhead at `rate` (1 by default). Sound plays at 1× only. */
  play: (rate?: number) => void;
  pause: () => void;
  toggle: () => void;
  /** J and L: start in `direction`, or double the speed when already going that way (2× at most). */
  shuttle: (direction: 1 | -1) => void;
  /** Back to the start without telling the engine (another project opened; the engine resets itself). */
  reset: () => void;
  onFrame: (frame: FrameMeta) => void;
  onState: (state: { playing: boolean; atUs: number; reason: StateReason; rate: number }) => void;
}

/** The open project's frame rate; `project.ts` registers it (it imports this store, so this can't import it). */
let frameRate = () => 30;
export function setFrameRateSource(source: () => number) {
  frameRate = source;
}

let pendingSeek: number | null = null;
let seekScheduled = false;
/** A frame `showFrameAt` asked for may still be waiting or on its way: no seek to the playhead has replaced it yet. */
let shownOut = false;
const nextFrame = (cb: () => void) =>
  typeof requestAnimationFrame === 'function' ? requestAnimationFrame(cb) : setTimeout(cb, 16);

/** Tells the engine where the playhead is, at most once per animation frame (a scrub sends the newest spot). */
export function sendSeek(us: number) {
  pendingSeek = us;
  if (seekScheduled) return;
  seekScheduled = true;
  nextFrame(() => {
    seekScheduled = false;
    const at = pendingSeek;
    pendingSeek = null;
    if (at !== null) void engine.seek(at, usePlayback.getState().seq).catch(() => undefined);
  });
}

/**
 * Shows the frame at `us` in the preview without moving the playhead or the timecode (a trim shows the edge it drags),
 * or the playhead's frame again (null). Each is a seek of its own, sent as the playhead's are: the newest once per
 * animation frame, under a new sequence, so the engine stops reading toward a still an older one asked for.
 */
export function showFrameAt(us: number | null) {
  shownOut = us !== null;
  usePlayback.setState((s) => ({ seq: s.seq + 1 }));
  sendSeek(us ?? usePlayback.getState().playheadUs);
}

/**
 * Playback starts from the playhead: a frame shown without moving it that still waits is dropped. True: one may be on
 * its way, so the run takes a new sequence, which makes that seek older than the run and the engine ignores it rather
 * than playing from it.
 */
function dropShown(): boolean {
  if (!shownOut) return false;
  shownOut = false;
  pendingSeek = null;
  return true;
}

export const usePlayback = create<PlaybackStore>((set, get) => ({
  playheadUs: 0,
  playing: false,
  rate: 1,
  seq: 0,
  seek: (us) => {
    const playheadUs = Math.max(0, Math.round(us));
    // While playing, the clock waits there for the first frame from the new spot.
    holdClock(playheadUs);
    shownOut = false;
    set((s) => ({ playheadUs, seq: s.seq + 1 }));
    sendSeek(playheadUs);
  },
  play: (rate = 1) => {
    if (get().playing) return;
    // The clock starts with the first playback frame (the sound may take a moment to open).
    holdClock(get().playheadUs);
    const fresh = dropShown();
    set((s) => ({ playing: true, rate, seq: fresh ? s.seq + 1 : s.seq }));
    engine.play(get().playheadUs, rate, get().seq).catch((error: unknown) => {
      set({ playing: false, rate: 1 });
      toast.error(errorMessage(error));
    });
  },
  pause: () => {
    if (!get().playing) return;
    const seq = get().seq;
    // The playhead and timecode stay where the clock is (not the last reported frame, which is a little behind) until
    // the engine says where it stopped.
    const at = Math.round(clockUs());
    holdClock(at);
    set({ playing: false, rate: 1, playheadUs: at });
    engine.pause().then(
      (atUs) => {
        // A seek or another project since then wins over where playback stopped.
        if (get().seq === seq && !get().playing) {
          holdClock(atUs);
          set({ playheadUs: atUs });
        }
      },
      () => undefined,
    );
  },
  toggle: () => (get().playing ? get().pause() : get().play()),
  shuttle: (direction) => {
    const { playing, rate } = get();
    // As `play()` does: the clock only while playing (its anchor is stale when stopped), else the playhead.
    const from = playing ? Math.round(clockUs()) : get().playheadUs;
    const next =
      playing && Math.sign(rate) === direction ? Math.min(2, Math.abs(rate) * 2) * direction : direction;
    if (playing && next === rate) return;
    // Nothing to play backward from the first frame.
    if (!playing && direction < 0 && from <= 0) return;
    holdClock(from);
    // A new run while playing: frames of the previous one still on their way carry the old sequence and are dropped.
    const fresh = dropShown() || playing;
    set((s) => ({ playing: true, rate: next, playheadUs: from, seq: fresh ? s.seq + 1 : s.seq }));
    engine.play(from, next, get().seq).catch((error: unknown) => {
      set({ playing: false, rate: 1 });
      toast.error(errorMessage(error));
    });
  },
  reset: () => {
    pendingSeek = null;
    shownOut = false;
    holdClock(0);
    set((s) => ({ playheadUs: 0, playing: false, rate: 1, seq: s.seq + 1 }));
  },
  onFrame: (frame) => {
    // Work waiting for the picture to catch up (a title typed on the canvas hands back to the preview's frames).
    noteFrameShown();
    const s = get();
    if (!s.playing || !frame.playback || frame.seq !== s.seq) return;
    // Only the clock's anchor moves the playhead's animation; the store follows for what depends on the scene (no DOM
    // write per frame). Backward, a frame is drawn as the clock enters it from above, at its last µs, and shows while
    // the clock runs down through it: the clock is anchored there, so it doesn't run a frame ahead of the picture. A
    // backward run that starts mid-frame (L→J, J J, a seek while going backward) starts from where the clock was held,
    // not from its first frame's last µs, or the playhead would sit ahead of the picture for the whole run.
    const fps = frameRate();
    const held = clockAnchor();
    const frameEndUs = frameStartUs(frameIndexAt(frame.timeUs, fps) + 1, fps) - 1;
    const anchorUs = s.rate >= 0 ? frame.timeUs : held.running ? frameEndUs : Math.min(frameEndUs, held.us);
    reportFrame(anchorUs, frame.atMs ?? performance.now(), s.rate);
    set({ playheadUs: frame.timeUs });
  },
  onState: ({ playing, atUs, rate }) => {
    // A stop the UI asked for already moved the playhead (pause); only the engine's own stops (end, hidden, error,
    // another project) and starts move it here.
    if (!playing && !get().playing) return;
    holdClock(atUs);
    set({ playing, rate: playing ? rate : 1, playheadUs: atUs });
  },
}));
