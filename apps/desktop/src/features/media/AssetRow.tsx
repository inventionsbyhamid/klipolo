import { useLingui } from '@lingui/react/macro';
import type { PointerEvent, ReactNode } from 'react';
import type { Asset } from '../../bindings/Asset';
import { Icon } from '../../layout/Icon';
import { formatPrecise, formatTime } from '../../lib/time';
import { beginMediaDrag } from '../timeline/drag/mediaDrag';
import type { DropTarget } from '../timeline/drops';

/** On a Mac, Control-click is a right-click: it doesn't start a drag. */
const MAC = typeof navigator !== 'undefined' && /^Mac/.test(navigator.platform);

/** The art on a row's left: a clip's 72 x 40 frame, or a 44 px tile in its lane's colour. */
export type RowArt = 'clip' | 'music' | 'sound';

const TILE_ICON = { clip: 'film', music: 'music', sound: 'pulse' } as const;

interface AssetRowProps {
  /** What it drags onto the timeline: its kind and length draw where it would land. */
  asset: Asset;
  art: RowArt;
  title: string;
  /** "On the timeline" when the video uses it. */
  used: boolean;
  /** The row's "+": adds it at the playhead (music: under the video from there). */
  onAdd: () => void;
  /** Let go on the timeline, at `target`. */
  onDrop: (target: DropTarget) => void;
  /** More actions, after the "+" (a clip's menu). */
  menu?: ReactNode;
  /** What the "+" says it does, when not adding at the playhead (a clip goes to the end). */
  addLabel?: string;
}

/**
 * One file in the Library (9.16): its art, its name and length (and "On the timeline" when the video uses
 * it), and a "+" that appears on hover or focus and adds it at the playhead. Pressed and moved, it drags onto the
 * timeline (`drag/mediaDrag`), where it lands on the lane under the pointer.
 */
export function AssetRow({ asset, art, title, used, onAdd, onDrop, menu, addLabel }: AssetRowProps) {
  const { t } = useLingui();
  const onPointerDown = (e: PointerEvent<HTMLLIElement>) => {
    // The buttons are their own controls.
    if (e.button !== 0 || (MAC && e.ctrlKey) || (e.target as Element).closest('button')) return;
    // No text selection and no native image drag; the row keeps the pointer wherever it goes.
    e.preventDefault();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    beginMediaDrag(e, asset, onDrop);
  };
  // A short sound to the tenth ("0:00.4", never under "0:00.1"), anything longer to the second.
  const length =
    asset.durationUs < 10_000_000 ? formatPrecise(Math.max(100_000, asset.durationUs)) : formatTime(asset.durationUs);
  return (
    <li
      data-asset-id={asset.id}
      className="row lib-row"
      onPointerDown={onPointerDown}
      onDragStart={(e) => e.preventDefault()}
    >
      <span className={art === 'clip' ? 'art-clip' : `art-tile is-${art}`} aria-hidden>
        <Icon name={TILE_ICON[art]} />
      </span>
      <span className="row-text">
        <span className="row-title" title={title}>
          {title}
        </span>
        <span className="row-meta">
          <span>{length}</span>
          {used && <span>{t({ id: 'library.used', message: 'On the timeline' })}</span>}
        </span>
      </span>
      <span className="row-actions">
        {menu}
        <button
          type="button"
          tabIndex={0}
          className="icon-btn row-action"
          aria-label={
            addLabel ??
            t({
              id: 'library.add',
              message: `Add ${title} at the playhead`,
              comment: '{title} is a file or a sound’s name',
            })
          }
          title={addLabel ?? t({ id: 'library.add.tip', message: 'Add at the playhead' })}
          onClick={onAdd}
        >
          <Icon name="plus" />
        </button>
      </span>
    </li>
  );
}
