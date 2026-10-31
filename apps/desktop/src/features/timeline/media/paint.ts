import type { ThumbStrip } from '../../../bindings/ThumbStrip';
import { pxToUs, usToPx } from '../scale';
import { placeDrawn, viewPps } from './anchor';
import { chunkFiles, drawFilmstrip, filmTiles, type FilmMap } from './drawFilmstrip';
import { drawWaveform, peakLevel, type WaveStyle } from './drawWaveform';
import { loadWave, type Wave } from './peaksCache';
import { resolveToken } from './resolveToken';
import { fileUs, runParts, runsKey, soundUs, sourceAtUs, type BlockRuns, type RunPart } from './runs';
import { spriteCache, stripOf } from './spriteCache';
import type { ZoomRest } from './zoomRest';

/** The blocks' 3 px left edge (their kind's colour): a canvas starts under it, so its x 0 is the block's start. */
const EDGE_PX = 3;
/**
 * The first sound (a recording's mic: the voice creators cut by) and the ones drawn lighter under it (system sound).
 */
const VOICE_ALPHA = 0.7;
const UNDER_ALPHA = 0.3;
/** Waveform heights in dB, from this far below full scale. */
const FLOOR_DB = 48;
/** Canvases wider than this (device px) draw at a lower resolution instead (WebKit's limit per side). */
const MAX_CANVAS_PX = 16_384;

/** What one block's canvas shows: its part [a, b) of the timeline (µs), and how that maps to its files. */
export interface PaintSpec {
  kind: 'clip' | 'video' | 'audio';
  /**
   * Its files: the filmstrip's runs (none: no filmstrip) and its sounds', the voice first (`media/runs`). A recording's
   * runs each play their own file at their own place and rate; an imported file is one run.
   */
  runs: BlockRuns;
  /** The media version to read of each film run's file, and of each sound run's (the sounds' runs in order). */
  filmVersions: readonly number[];
  soundVersions: readonly number[];
  a: number;
  b: number;
  /**
   * The source time a zoomed-out filmstrip sizes its chunk groups from: the lanes' window at this zoom (µs), the same
   * at every window step; each run takes it at its rate, at most its file's length.
   */
  spanUs: number;
  startUs: number;
  durationUs: number;
  sourceInUs: number;
  speed: number;
  volume: number;
  fadeInUs: number;
  fadeOutUs: number;
  muted: boolean;
  pps: number;
  /** The canvas's height, the filmstrip's from its top and the waveform's at its bottom (CSS px). */
  height: number;
  filmPx: number;
  wavePx: number;
  /** The colour token its waveform draws in (a clip's, the Music lane's, the Sounds lane's). */
  waveToken: string;
  theme: number;
}

/** Every block's draws in one animation frame; a block draws at most once a frame. */
const queued = new Set<() => void>();
let frame = 0;
function inFrame(draw: () => void) {
  queued.add(draw);
  frame ||= requestAnimationFrame(() => {
    frame = 0;
    const draws = [...queued];
    queued.clear();
    for (const run of draws) run();
  });
}

/** What each canvas shows, so a redraw that would draw the same (a media event about another chunk) is skipped. */
const shown = new WeakMap<HTMLCanvasElement, string>();

/**
 * Loads what `spec` needs (the strip and its sprites, the peaks) and draws it in an animation frame, once any zoom
 * under way has rested (`rest`). Every load is fire-and-forget: a failure leaves that part in the block's plain colour
 *.
 *
 * A canvas that already shows something keeps it, over the time it drew, until everything has arrived, so a zoom or a
 * window step never flashes; a new one draws each part as soon as it exists (peaks come long before thumbnails). The
 * canvas takes its new place as it draws. Once nothing is pending it lets go of what it loaded (the canvas keeps the
 * picture; the caches keep the newest). The function it returns drops results still on their way.
 */
export function paintBlock(canvas: HTMLCanvasElement, spec: PaintSpec, rest: ZoomRest | null): () => void {
  const { a, b, startUs, durationUs, sourceInUs, speed, pps, height, filmPx, wavePx } = spec;
  const widthPx = usToPx(b - a, pps);
  const scale = Math.min(window.devicePixelRatio || 1, 2, MAX_CANVAS_PX / Math.max(1, widthPx));
  const w = Math.max(1, Math.round(widthPx * scale));
  const h = Math.max(1, Math.round(height * scale));
  const clock = { sourceInUs, speed, pps };
  const sourceAt = (us: number) => sourceAtUs(clock, startUs, us);
  const lastSource = sourceAt(startUs + durationUs) - 1;
  const x0 = usToPx(a - startUs, pps);
  const blockSource = (x: number) => sourceAt(startUs + pxToUs(x, pps));
  /** A film run's map over its part: its own time, held inside the block's footage and its file. */
  const filmMap = (part: RunPart): FilmMap => {
    const { run } = part;
    // A run that starts inside its file (a camera paused and resumed) shows its file from there.
    const inUs = run.inUs ?? 0;
    const end = Math.min(run.fileUs, inUs + (run.toUs - run.fromUs) * run.rate) - 1;
    const held = (us: number) => Math.min(Math.max(fileUs(run, us), inUs), end);
    const lo = held(Math.max(sourceInUs, run.fromUs));
    const hi = Math.max(lo, held(Math.min(lastSource, run.toUs - 1)));
    return {
      x0: part.xa,
      widthPx: part.xb - part.xa,
      heightPx: filmPx,
      blockPx: usToPx(durationUs, pps),
      spanUs: Math.min(spec.spanUs * run.rate, run.fileUs),
      usPerPx: pxToUs(1, pps) * speed * run.rate,
      sourceAt: (x) => Math.min(Math.max(fileUs(run, blockSource(x)), lo), hi),
    };
  };
  const films = runParts(spec.runs.film, clock, x0, x0 + widthPx, true);
  // Sound plays nothing between runs: each track's parts, the voice's first.
  const sounds = spec.runs.sounds.map((track) => runParts(track, clock, x0, x0 + widthPx, false));
  const gainAt = (x: number) => {
    if (spec.muted) return 0;
    const into = a - startUs + pxToUs((x + 0.5) / scale, pps);
    const fadeIn = spec.fadeInUs > 0 ? into / spec.fadeInUs : 1;
    const fadeOut = spec.fadeOutUs > 0 ? (durationUs - into) / spec.fadeOutUs : 1;
    return spec.volume * Math.max(0, Math.min(1, fadeIn, fadeOut));
  };
  // What the picture depends on besides the loaded files (sprites and peaks never change once written).
  const look = [spec.kind, a, b, startUs, durationUs, sourceInUs, speed, pps, height, filmPx, wavePx, scale]
    .concat([spec.spanUs, spec.volume, spec.fadeInUs, spec.fadeOutUs, Number(spec.muted), spec.theme])
    .join()
    .concat('|', spec.waveToken, '|', runsKey(spec.runs));

  let live = true;
  let pending = 0;
  /** Each film part's strip and decoded sprites (by chunk), in `films`' order. */
  const strips: (ThumbStrip | null)[] = films.map(() => null);
  const images = films.map(() => new Map<number, HTMLImageElement>());
  const sprites: string[] = [];
  /** Each sound part's peaks, track by track. */
  let waves: (Wave | null)[][] = sounds.map((parts) => parts.map(() => null));

  const drawWave = (
    ctx: CanvasRenderingContext2D,
    part: RunPart,
    wave: Wave,
    alpha: number,
    gain: number,
    channel: number | null,
  ) => {
    const style: WaveStyle = {
      color: resolveToken(spec.waveToken),
      baseline: spec.kind === 'clip' ? 'bottom' : 'centre',
      floorDb: FLOOR_DB,
    };
    // Its columns from the canvas's column at its start.
    const offset = Math.round((part.xa - x0) * scale);
    const waveMap = {
      columns: Math.min(w - offset, Math.round((part.xb - part.xa) * scale)),
      bucketsPerColumn: (wave.rate * speed * part.run.rate) / (pps * scale),
      firstBucket: (soundUs(part.run, blockSource(part.xa)) / 1e6) * wave.rate - wave.from,
      height: Math.round(wavePx * scale),
      gainAt: (x: number) => gainAt(x + offset) * gain,
      channel,
    };
    ctx.setTransform(1, 0, 0, 1, offset, Math.round((height - wavePx) * scale));
    ctx.globalAlpha = alpha;
    drawWaveform(ctx, wave.peaks, wave.channels, waveMap, style);
  };
  const drawFilm = (ctx: CanvasRenderingContext2D, k: number) => {
    const strip = strips[k];
    if (!strip) return;
    const part = films[k];
    ctx.setTransform(scale, 0, 0, scale, (part.xa - x0) * scale, 0);
    // A run that covers only part of the canvas keeps to it: its edge tiles are cut where the next run starts.
    const whole = part.xa === x0 && part.xb === x0 + widthPx;
    if (!whole) {
      ctx.save();
      ctx.beginPath();
      ctx.rect(0, 0, part.xb - part.xa, filmPx);
      ctx.clip();
    }
    drawFilmstrip(ctx, strip, images[k], filmMap(part));
    if (!whole) ctx.restore();
  };
  const paint = (ctx: CanvasRenderingContext2D) => {
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    } else {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, w, h);
    }
    films.forEach((_, k) => drawFilm(ctx, k));
    // The voice over the rest: system sound reads as a lighter shape behind the speech.
    // Each sound at its volume in the clip's mix (a muted one isn't in `runs`), from the channel it plays if centred.
    const { gains, centres } = spec.runs;
    const track = (t: number, alpha: number) =>
      sounds[t].forEach(
        (part, k) =>
          waves[t][k] && drawWave(ctx, part, waves[t][k]!, alpha, gains?.[t] ?? 1, centres?.[t] ?? null),
      );
    for (let t = 1; t < sounds.length; t++) track(t, UNDER_ALPHA);
    if (sounds.length > 0) track(0, VOICE_ALPHA);
    ctx.globalAlpha = 1;
    // The canvas takes its new part's place as the picture changes, never before (`anchorCanvas`).
    canvas.style.left = `${x0 - EDGE_PX}px`;
    canvas.style.width = `${widthPx}px`;
    placeDrawn(canvas, { offUs: a - startUs, srcUs: sourceAt(a), pps, speed });
  };
  const draw = () => {
    if (!live) return;
    const before = shown.get(canvas);
    const hasWave = waves.some((track) => track.some(Boolean));
    // A zoom moved on since this started (its own paint replaces it): the canvas stays anchored as it is.
    const stale = (viewPps(canvas) ?? pps) !== pps;
    if (!stale && (before === undefined ? strips.some(Boolean) || hasWave : pending === 0)) {
      const waveKeys = waves.map((track) => track.map((wave) => wave?.key).join()).join('|');
      const key = [look, [...sprites].sort().join(), waveKeys].join('|');
      const ctx = key === before ? null : canvas.getContext('2d');
      if (ctx) {
        shown.set(canvas, key);
        paint(ctx);
      }
    }
    if (pending > 0 && !stale) return;
    // Done. React keeps the cleanup function, and with it this scope, until the next paint or an unmount: let go of
    // the sprites and peaks now rather than with the closure.
    live = false;
    strips.fill(null);
    waves = [];
    for (const byChunk of images) byChunk.clear();
  };
  const track = <T>(load: Promise<T>, then: (value: T) => void) => {
    pending++;
    load
      .then((value) => live && then(value))
      .catch(() => undefined)
      .finally(() => {
        pending--;
        if (live) inFrame(draw);
      });
  };

  const loadFilm = (k: number) => {
    const part = films[k];
    track(stripOf(part.run.path, spec.filmVersions[part.index] ?? 0), (found) => {
      if (!found) return;
      strips[k] = found;
      const files = chunkFiles(found);
      // At most MAX_SPRITES chunks a file (`filmTiles`), so a paint never holds more decoded sprites than that.
      for (const chunk of new Set(filmTiles(found, filmMap(part)).map((tile) => tile.chunk))) {
        const path = files.get(chunk);
        if (path === undefined) continue;
        const add = (img: HTMLImageElement) => {
          images[k].set(chunk, img);
          sprites.push(path);
        };
        const hit = spriteCache.image(path);
        if (hit) add(hit);
        else
          track(
            spriteCache.load(path, () => live),
            add,
          );
      }
    });
  };
  const begin = () => {
    films.forEach((_, k) => loadFilm(k));
    // Each sound run's file version, as `soundVersions` lists them: the tracks' runs in order.
    let version = 0;
    sounds.forEach((parts, t) => {
      const first = version;
      version += spec.runs.sounds[t].length;
      parts.forEach((part, k) => {
        const level = peakLevel(pps / (speed * part.run.rate), scale);
        const from = soundUs(part.run, blockSource(part.xa));
        const to = soundUs(part.run, blockSource(part.xb));
        const v = spec.soundVersions[first + part.index] ?? 0;
        track(loadWave(part.run.path, v, from, to, level), (found) => {
          waves[t][k] = found;
        });
      });
    });
  };
  let cancel: () => void = () => undefined;
  if (rest) cancel = rest.whenRested(pps, begin);
  else begin();
  return () => {
    live = false;
    cancel();
  };
}
