import { useLingui } from '@lingui/react/macro';
import { memo, useRef, type KeyboardEvent, type MouseEvent } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { Marker } from '../../bindings/Marker';
import type { Recording } from '../../bindings/Recording';
import { formatTime, snapToFrame } from '../../lib/time';
import { playedUs } from '../../lib/timeline';
import { usePlayback } from '../../state/playback';
import { useProposal, type Proposal } from '../../state/proposal';
import { removeMarker } from './keys/sourceActions';
import { mainIndex } from './model/index';
import { usToPx } from './scale';

/**
 * Flags closer than a flag's width (px) to the one drawn before them aren't drawn: zoomed out, an hour of markers
 * mounts at most a flag per 10 px of lane and stays legible.
 */
const MIN_GAP_PX = 10;

/** Where each marker plays on the video, in time order: a marker plays once in every clip that holds its moment. */
interface Flags {
  at: Float64Array;
  recording: string[];
  marker: Marker[];
}

/** A recording's markers in time order (the model keeps them so; a hand-made file may not): sorted once if not. */
const ordered = new WeakMap<Marker[], Marker[]>();
function inOrder(markers: Marker[]): Marker[] {
  let out = ordered.get(markers);
  if (!out) {
    const sorted = markers.every((m, i) => i === 0 || markers[i - 1].atUs <= m.atUs);
    ordered.set(markers, (out = sorted ? markers : [...markers].sort((a, b) => a.atUs - b.atUs)));
  }
  return out;
}

/**
 * The flags of `main`'s clips, built once per main track and recording list (a patch keeps both when neither changed):
 * a binary search per recording clip into its recording's markers, then one step per marker it plays. Clips are in
 * timeline order and a clip's markers in its own, so the list comes out sorted. O(clips × log markers + flags).
 */
const built = new WeakMap<Clip[], WeakMap<Recording[], Flags>>();
function flagsOf(main: Clip[], recordings: Recording[]): Flags {
  let byRecordings = built.get(main);
  if (!byRecordings) built.set(main, (byRecordings = new WeakMap()));
  let flags = byRecordings.get(recordings);
  if (flags) return flags;
  const markers = new Map(recordings.map((r) => [r.id, inOrder(r.markers)]));
  const { starts } = mainIndex(main);
  const at: number[] = [];
  const recording: string[] = [];
  const marker: Marker[] = [];
  main.forEach((clip, i) => {
    const source = clip.source;
    const list = source.type === 'recording' ? markers.get(source.id) : undefined;
    if (!list || list.length === 0) return;
    let lo = 0;
    let hi = list.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (list[mid].atUs < clip.sourceInUs) lo = mid + 1;
      else hi = mid;
    }
    for (let k = lo; k < list.length && list[k].atUs < clip.sourceOutUs; k++) {
      at.push(starts[i] + playedUs(list[k].atUs - clip.sourceInUs, clip.speed));
      recording.push(source.id);
      marker.push(list[k]);
    }
  });
  flags = { at: Float64Array.from(at), recording, marker };
  byRecordings.set(recordings, flags);
  return flags;
}

/** The markers a waiting proposal removes, by id. Built once per proposal. */
const ghosts = new WeakMap<Proposal, Set<string>>();
function ghostedBy(proposal: Proposal): Set<string> {
  let ids = ghosts.get(proposal);
  if (!ids) ghosts.set(proposal, (ids = new Set(proposal.removals.markers)));
  return ids;
}

interface MarkerFlagsProps {
  recordings: Recording[];
  main: Clip[];
  fps: number;
  pps: number;
  fromUs: number;
  toUs: number;
}

/**
 * A recording's markers as flags along the top of the Video lane, where they play on the video, each with a line down
 * through the clips that takes no presses. Only the window's are drawn (a binary search), and those closer than 10 px
 * to the one before are left out, so an hour of markers zoomed out stays cheap and legible. A click seeks to the marker
 * (and focuses its flag); the arrows move between flags; Enter or Space seeks; Delete removes the focused one, the
 * person's own edit, in one undo step. A marker a waiting chat proposal removes is drawn as going, in red.
 */
export const MarkerFlags = memo(function MarkerFlags({ recordings, main, fps, pps, fromUs, toUs }: MarkerFlagsProps) {
  const { t } = useLingui();
  const proposal = useProposal((s) => s.proposal);
  const root = useRef<HTMLDivElement>(null);
  const flags = flagsOf(main, recordings);
  const { at } = flags;
  if (at.length === 0) return null;
  const ghosted = proposal ? ghostedBy(proposal) : null;
  let i = 0;
  let hi = at.length;
  while (i < hi) {
    const mid = (i + hi) >>> 1;
    if (at[mid] < fromUs) i = mid + 1;
    else hi = mid;
  }
  const seek = (us: number) => usePlayback.getState().seek(snapToFrame(us, fps));
  const focusFlag = (key: string | null) =>
    key && root.current?.querySelector<HTMLElement>(`[data-marker="${CSS.escape(key)}"]`)?.focus();
  const shown = [];
  let last = -Infinity;
  for (; i < at.length && at[i] < toUs; i++) {
    const left = usToPx(at[i], pps);
    if (left - last < MIN_GAP_PX) continue;
    last = left;
    const [us, recording, marker] = [at[i], flags.recording[i], flags.marker[i]];
    const time = formatTime(us);
    const named = marker.label?.trim();
    const name = named
      ? t({
          id: 'marker.flagNamed',
          message: `${named}, marker at ${time}`,
          comment: "A marker's flag on the timeline, for screen readers; {named} is the person's own label",
        })
      : t({
          id: 'marker.flag',
          message: `Marker at ${time}`,
          comment: "A marker's flag on the timeline, for screen readers; {time} is where it plays, e.g. 2:05",
        });
    const ghost = ghosted?.has(marker.id) ?? false;
    const key = `${recording}:${marker.id}`;
    const onClick = (e: MouseEvent<HTMLButtonElement>) => {
      // Never reaches the lane, which would seek to the pointer and deselect.
      e.stopPropagation();
      // WebKit doesn't focus a pressed button: Delete then reaches this flag.
      e.currentTarget.focus({ preventScroll: true });
      seek(us);
    };
    const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
      // Every child of the flags' layer is a flag, in time order.
      const { nextElementSibling: after, previousElementSibling: before } = e.currentTarget;
      if (e.key === 'Delete' || e.key === 'Backspace') {
        // The window's Delete (the selected clip) doesn't also run.
        e.preventDefault();
        e.stopPropagation();
        // Focus goes on to the flag after it (else the one before) once it has gone.
        const then = ((after ?? before) as HTMLElement | null)?.dataset.marker ?? null;
        removeMarker(recording, marker.id, () => requestAnimationFrame(() => focusFlag(then)));
      } else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
        e.preventDefault();
        e.stopPropagation();
        ((e.key === 'ArrowRight' ? after : before) as HTMLElement | null)?.focus();
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        e.stopPropagation();
        seek(us);
      }
    };
    shown.push(
      <button
        key={`${key}:${i}`}
        type="button"
        className="tl-marker"
        data-marker={key}
        data-ghost={ghost || undefined}
        aria-label={name}
        title={named || t({ id: 'marker.tip', message: 'Marker', comment: "A marker's tooltip when it has no label" })}
        tabIndex={shown.length === 0 ? 0 : -1}
        onClick={onClick}
        onKeyDown={onKeyDown}
        // Space's release would click it again (WebKit clicks a button on keyup).
        onKeyUp={(e) => e.key === ' ' && e.preventDefault()}
        style={{ left: left - 1 }}
      >
        <span aria-hidden className="tl-marker-flag" />
        {/* Its line down through the clips, which takes no presses: a press there reaches the clip. */}
        <span aria-hidden className="tl-marker-line" />
      </button>,
    );
  }
  if (shown.length === 0) return null;
  return (
    <div ref={root} className="tl-markers">
      {shown}
    </div>
  );
});
