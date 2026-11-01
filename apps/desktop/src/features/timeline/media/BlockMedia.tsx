import { memo, useContext, useEffect, useLayoutEffect, useMemo, useRef, useSyncExternalStore } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { cn } from '../../../design/cn';
import { useMedia } from '../../../state/media';
import { CLIP_INNER_PX, FILM_SHARE, gutterPx } from '../layout';
import { windowSpanUs } from '../useViewport';
import { anchorCanvas } from './anchor';
import { paintBlock } from './paint';
import { onThemeChange, themeVersion } from './resolveToken';
import { wholeFile, type BlockRuns, type FileRuns } from './runs';
import { ViewportContext, ZoomRestContext } from './ViewportContext';

/** A video layer's filmstrip needs a row this tall; an overlay row (18 px) shows its colour. */
const FILM_MIN_PX = 24;
/** What a waveform draws in, by kind: a clip's own sound in the clip's lighter blue. */
const WAVE_TOKEN = { clip: '--color-clip-2', video: '--color-clip-2', audio: '--color-lane-sfx' } as const;
const NO_RUNS: FileRuns = [];
const NO_SOUNDS: readonly FileRuns[] = [];
/**
 * While a start edge is dragged (`drag/registry` `sizeBlock`), the block's left moves by `--trim-dx`: the picture moves
 * back by as much, so it stays put in time and the edge passes over it (trim to where the speech starts).
 */
const TRIM_COUNTER = 'calc(0px - var(--trim-dx, 0px)) 0';

interface BlockMediaProps {
  kind: 'clip' | 'video' | 'audio';
  /** Clips: the asset's or recording's screen, else camera; video layers: their asset; none for sound. */
  thumbPath?: string;
  /** That file's length (µs). */
  filmUs?: number;
  /** Clips: the asset, or the recording's mic then system sound; sound layers: their asset. Memo-stable. */
  peakPaths: readonly string[];
  /** The sound lead of the file `peakPaths` draws (an imported video's): its peaks are read at file time less it. */
  peakLeadUs?: number;
  /**
   * A recording's files run by run, each at its place and rate on the recording clock (`media/blockFiles`;
   * memo-stable), instead of `thumbPath` and `peakPaths` played whole from source time 0.
   */
  runs?: BlockRuns;
  /** A sound layer's channel it plays centred from (`ItemContent`'s `centreFrom`): its waveform draws that one. */
  centreFrom?: number | null;
  startUs: number;
  durationUs: number;
  sourceInUs: number;
  speed: number;
  volume: number;
  fadeInUs: number;
  fadeOutUs: number;
  /** Silent: its sound draws as a flat line (a muted track, a muted video). */
  muted: boolean;
  /** Its sound keeps its shape, drawn faded: a sound layer muted by its own switch (CSS opacity, so no redraw). */
  faded?: boolean;
  pps: number;
  /** The block's inside (CSS px); a clip's by default. */
  height?: number;
  /** The colour token its waveform draws in; by default its kind's (`WAVE_TOKEN`). */
  waveToken?: string;
}

/**
 * A block's pictures and sound, under its labels: one canvas over only its part inside the lanes' window, so an
 * hour-long clip never makes a huge canvas. A clip shows its filmstrip over its top 62 % and its waveform along the
 * bottom 38 % (the voice over a lighter system sound), a sound layer a full-height waveform, and a video layer its
 * filmstrip where its row is tall enough.
 *
 * It redraws (`paintBlock`) when that part changes (a window step across the block, a zoom once it rests), on a theme
 * or pixel-density change, and on a media event for one of its files. Until then the canvas stays over the time it
 * drew (`anchorCanvas`), stretched with a zoom. Nothing redraws while a drag moves or trims its block, nor as the
 * playhead moves. What isn't ready yet, or can't load, shows the block's plain colour.
 */
export const BlockMedia = memo(function BlockMedia({
  kind,
  thumbPath,
  filmUs,
  peakPaths,
  peakLeadUs = 0,
  runs: recorded,
  centreFrom = null,
  startUs,
  durationUs,
  sourceInUs,
  speed,
  volume,
  fadeInUs,
  fadeOutUs,
  muted,
  faded = false,
  pps,
  height = CLIP_INNER_PX,
  waveToken = WAVE_TOKEN[kind],
}: BlockMediaProps) {
  const { fromUs, toUs, widthPx } = useContext(ViewportContext);
  const rest = useContext(ZoomRestContext);
  // A clip: its filmstrip over the top 62 %, its own waveform along the bottom 38 %.
  const filmPx =
    kind === 'clip' ? Math.round(height * FILM_SHARE) : kind === 'video' && height >= FILM_MIN_PX ? height : 0;
  const wavePx = kind === 'clip' ? height - filmPx : kind === 'audio' ? height : 0;
  // What it draws, as runs (an imported file is one): only what its height has room for.
  const runs = useMemo((): BlockRuns => {
    const all: BlockRuns = recorded ?? {
      film: thumbPath ? [wholeFile(thumbPath, filmUs || Infinity)] : NO_RUNS,
      sounds: peakPaths.map((path) => [wholeFile(path, Infinity, peakLeadUs)]),
      ...(centreFrom !== null && { centres: peakPaths.map(() => centreFrom) }),
    };
    return { ...all, film: filmPx > 0 ? all.film : NO_RUNS, sounds: wavePx > 0 ? all.sounds : NO_SOUNDS };
  }, [recorded, thumbPath, filmUs, peakPaths, peakLeadUs, centreFrom, filmPx, wavePx]);
  const hasMedia = runs.film.length > 0 || runs.sounds.length > 0;
  const filmVersions = useMedia(useShallow((s) => runs.film.map((run) => s.versions[run.path] ?? 0)));
  const soundVersions = useMedia(
    useShallow((s) => runs.sounds.flatMap((track) => track.map((run) => s.versions[run.path] ?? 0))),
  );
  const theme = useSyncExternalStore(onThemeChange, themeVersion);
  const ref = useRef<HTMLCanvasElement>(null);
  // Its part inside the window. Out of it (a kept block: selected, focused, dragged), it keeps what it drew.
  const a = Math.max(startUs, fromUs);
  const b = Math.min(startUs + durationUs, toUs);
  // How far a zoomed-out filmstrip's chunk groups reach: the window's length at this zoom in source time (the same at
  // every step); each run takes at most its file's.
  const spanUs = windowSpanUs(widthPx, pps, gutterPx()) * speed;

  // Before the browser paints a zoom's or an edit's render: the old picture follows its time.
  useLayoutEffect(() => {
    if (ref.current) anchorCanvas(ref.current, { pps, sourceInUs, speed });
  }, [pps, sourceInUs, speed, hasMedia]);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || b <= a) return;
    const what = { kind, runs, filmVersions, soundVersions, a, b, spanUs, startUs, durationUs };
    const how = { sourceInUs, speed, volume, fadeInUs, fadeOutUs, muted, pps, height, filmPx, wavePx, theme };
    const look = { waveToken };
    return paintBlock(canvas, { ...what, ...how, ...look }, rest);
  }, [
    kind,
    runs,
    filmVersions,
    soundVersions,
    a,
    b,
    spanUs,
    startUs,
    durationUs,
    sourceInUs,
    speed,
    volume,
    fadeInUs,
    fadeOutUs,
    muted,
    pps,
    height,
    filmPx,
    wavePx,
    waveToken,
    theme,
    rest,
  ]);

  if (!hasMedia) return null;
  // Placed (left, width) as it draws: until then it stays over the time it last drew.
  return (
    <canvas
      ref={ref}
      aria-hidden
      className={cn('pointer-events-none absolute top-0 origin-top-left', faded && 'opacity-40')}
      style={{ height, translate: TRIM_COUNTER }}
    />
  );
});
