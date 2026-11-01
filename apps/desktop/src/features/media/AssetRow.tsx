import { useLingui } from '@lingui/react/macro';
import type { PointerEvent, ReactNode } from 'react';
import type { Asset } from '../../bindings/Asset';
import { Icon } from '../../layout/Icon';
import { formatRowLength } from '../../lib/time';
import { beginMediaDrag } from '../timeline/drag/mediaDrag';
import type { DropTarget } from '../timeline/drops';
import { FavoriteButton, type Favorite } from './FavoriteButton';

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
  /** Its star, after the "+". */
  favorite?: Favorite;
  /** What its meta line says before "On the timeline", in place of its length (a recording's: "1:00, screen and mic"). */
  meta?: string;
  /**
   * The recording the row is, by its id: the row is that whole take, not `asset` alone (which only says how its drag
   * draws), and it drags onto the Video lane only.
   */
  recording?: string;
}

/**
 * One file in the Library, or one recording as a whole: its art, its name and length (and "On the
 * timeline" when the video uses it), and a "+" that appears on hover or focus and adds it at the playhead, then its
 * star (shown while it is a favorite). Pressed and moved, it drags onto the timeline (`drag/mediaDrag`), where it
 * lands on the lane under the pointer.
 */
export function AssetRow({
  asset,
  art,
  title,
  used,
  onAdd,
  onDrop,
  menu,
  addLabel,
  favorite,
  meta,
  recording,
}: AssetRowProps) {
  const { t } = useLingui();
  const onPointerDown = (e: PointerEvent<HTMLLIElement>) => {
    // The buttons are their own controls.
    if (e.button !== 0 || (MAC && e.ctrlKey) || (e.target as Element).closest('button')) return;
    // No text selection and no native image drag; the row keeps the pointer wherever it goes.
    e.preventDefault();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    beginMediaDrag(e, asset, onDrop, recording ? 'main' : 'any');
  };
  return (
    <li
      data-asset-id={recording ? undefined : asset.id}
      data-recording-id={recording}
      className={favorite?.on ? 'row lib-row is-favorite' : 'row lib-row'}
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
          <span title={meta}>{meta ?? formatRowLength(asset.durationUs)}</span>
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
        {favorite && <FavoriteButton title={title} favorite={favorite} />}
      </span>
    </li>
  );
}
