import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, type MouseEvent, type PointerEvent } from 'react';
import { Icon } from '../../layout/Icon';
import { useProject } from '../../state/project';
import { beginMediaDrag } from '../timeline/drag/mediaDrag';
import { DRAG_SLOP_PX } from '../timeline/drag/session';
import { addClip, toTheEnd } from './addMedia';
import { squareTileStyle, useClipPicture } from './clipPicture';
import { loadMarks, marked, markUsed, useSettledMarks } from './marks';
import { shelves, useNearView, type ShelfClip } from './shelves';

/** On a Mac, Control-click is a right-click: it doesn't start a drag. */
const MAC = typeof navigator !== 'undefined' && /^Mac/.test(navigator.platform);

/**
 * One clip's thumbnail on the folded library's rail: a button named by the clip (its tooltip too), its first frame
 * over the clip art (which shows until the frame is here). A press adds it to the end, as its row's "+" does; pressed
 * and moved, it drags onto the timeline as its row does (a whole recording onto the Video lane only), and the press
 * that let go of a drag (wherever it was let go, or put down with Escape) adds nothing more.
 */
function RailClip({ clip }: { clip: ShelfClip }) {
  const art = useRef<HTMLSpanElement>(null);
  const tile = useClipPicture(clip.asset.path, useNearView(art));
  /** The press under way: where it began, and whether it went past the slop (so it lifted into a drag). */
  const press = useRef<{ x: number; y: number; lifted: boolean } | null>(null);
  const add = (target: Parameters<typeof addClip>[1]) =>
    addClip(clip.source, target, () => clip.key && markUsed(clip.key));
  const past = (e: { clientX: number; clientY: number }, from: { x: number; y: number }) =>
    Math.hypot(e.clientX - from.x, e.clientY - from.y) >= DRAG_SLOP_PX;
  const onPointerDown = (e: PointerEvent<HTMLButtonElement>) => {
    press.current = null;
    if (e.button !== 0 || (MAC && e.ctrlKey)) return;
    press.current = { x: e.clientX, y: e.clientY, lifted: false };
    // No text selection and no native image drag; the button keeps the pointer wherever it goes.
    e.preventDefault();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    beginMediaDrag(e, clip.asset, add, clip.source.type === 'recording' ? 'main' : 'any');
  };
  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    const now = press.current;
    if (now && !now.lifted && past(e, now)) now.lifted = true;
  };
  const onClick = (e: MouseEvent<HTMLButtonElement>) => {
    const from = press.current;
    press.current = null;
    // Control-click on a Mac is a right-click: it adds nothing.
    if (MAC && e.ctrlKey) return;
    // From the keyboard (no pointer), or a press that never became a drag.
    const dragged = e.detail > 0 && from !== null && (from.lifted || past(e, from));
    if (!dragged) add(toTheEnd);
  };
  return (
    <li>
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        className="rail-clip"
        aria-label={clip.name}
        title={clip.name}
        data-recording-id={clip.source.type === 'recording' ? clip.source.id : undefined}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onClick={onClick}
        onDragStart={(e) => e.preventDefault()}
      >
        <span ref={art} className="rail-art" aria-hidden>
          <Icon name="film" />
          {tile && <span className="rail-tile" style={squareTileStyle(tile)} />}
        </span>
      </button>
    </li>
  );
}

/**
 * The folded library's clips (a 72 px rail of clip thumbnails), in the Clips tab's order: a recording as one
 * (its main footage's picture), then the videos, favorites and the recently used first. As the list does, it moves
 * no thumbnail while the pointer or the focus is on it (`useSettledMarks`), so a second press lands on the clip aimed
 * at. Nothing when the project has no clips.
 */
export function ClipRail() {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const order = useSettledMarks('rail');
  useEffect(loadMarks, []);
  if (!project) return null;
  const clips = marked(shelves(project).clips, (clip) => clip.key ?? '', order.marks);
  if (clips.length === 0) return null;
  return (
    <ul
      className="rail-clips"
      aria-label={t({ id: 'library.clips', message: 'Your clips' })}
      onPointerLeave={order.settle}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget)) order.settle();
      }}
    >
      {clips.map((clip) => (
        <RailClip key={`${clip.source.type}:${clip.source.id}`} clip={clip} />
      ))}
    </ul>
  );
}
