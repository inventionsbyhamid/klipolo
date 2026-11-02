import { useLingui } from '@lingui/react/macro';
import { isValidElement, memo, type PointerEvent, type ReactNode } from 'react';
import type { Asset } from '../../bindings/Asset';
import { cn } from '../../design/cn';
import { Icon } from '../../layout/Icon';
import { countRender } from '../../lib/renderCount';
import { formatRowLength } from '../../lib/time';
import { beginMediaDrag } from '../timeline/drag/mediaDrag';
import type { DropTarget } from '../timeline/drops';
import { FavoriteButton, type Favorite } from './FavoriteButton';
import { PREVIEW_MAX_US } from './preview';

/** On a Mac, Control-click is a right-click: it doesn't start a drag. */
const MAC = typeof navigator !== 'undefined' && /^Mac/.test(navigator.platform);

/** The art on a row's left: a clip's 72 x 40 frame, or a 44 px tile in its lane's colour. */
export type RowArt = 'clip' | 'music' | 'sound';

const TILE_ICON = { clip: 'film', music: 'music', sound: 'pulse' } as const;

/** A sound row's preview (`preview.ts`): whether it is asked to play, whether it is heard yet, and its play button. */
export interface RowPreview {
  /** Pressed: playing, or about to (its button stops it). */
  on: boolean;
  /** Heard: the row's progress runs. */
  sounding: boolean;
  toggle: () => void;
}

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
  /** Its play button, over its art: a sound or music row hears it before it is added. */
  preview?: RowPreview;
}

/** The same fields with the same values: an asset rebuilt from the same project reads the same. */
function sameAsset(a: Asset, b: Asset): boolean {
  if (a === b) return true;
  const keys = Object.keys(a) as (keyof Asset)[];
  return keys.length === Object.keys(b).length && keys.every((k) => a[k] === b[k]);
}

/** The asset a row's menu is for (`AssetMenu`), if it has one. */
const menuAsset = (menu: ReactNode) => (isValidElement<{ asset?: Asset }>(menu) ? menu.props.asset : undefined);

/**
 * Whether a row shows the same: its callbacks and its menu are made from these same fields, so a row whose fields are
 * unchanged acts the same with the ones it has, and an edit elsewhere (or to another row) doesn't draw it again.
 */
function sameRow(a: AssetRowProps, b: AssetRowProps): boolean {
  const menuA = menuAsset(a.menu);
  const menuB = menuAsset(b.menu);
  return (
    sameAsset(a.asset, b.asset) &&
    a.art === b.art &&
    a.title === b.title &&
    a.used === b.used &&
    a.addLabel === b.addLabel &&
    a.meta === b.meta &&
    a.recording === b.recording &&
    !!a.preview === !!b.preview &&
    a.preview?.on === b.preview?.on &&
    a.preview?.sounding === b.preview?.sounding &&
    !!a.favorite === !!b.favorite &&
    a.favorite?.on === b.favorite?.on &&
    !!a.menu === !!b.menu &&
    (menuA === menuB || (!!menuA && !!menuB && sameAsset(menuA, menuB)))
  );
}

/**
 * One file in the Library, or one recording as a whole: its art, its name and length (and "On the
 * timeline" when the video uses it), and a "+" that appears on hover or focus and adds it at the playhead, then its
 * star (shown while it is a favorite). A sound or music row's art is its play button: it shows play on hover or focus,
 * and stop while it plays, with a thin bar along the row's foot for how far it has played. Pressed and moved, it drags onto the timeline (`drag/mediaDrag`), where it
 * lands on the lane under the pointer.
 */
export const AssetRow = memo(function AssetRow({
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
  preview,
}: AssetRowProps) {
  countRender(`row:${recording ?? asset.id}`);
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
      className={cn('row lib-row', favorite?.on && 'is-favorite', preview?.on && 'is-previewing')}
      onPointerDown={onPointerDown}
      onDragStart={(e) => e.preventDefault()}
    >
      {preview && art !== 'clip' ? (
        <button
          type="button"
          tabIndex={0}
          className={`art-tile is-${art} preview-play`}
          aria-label={
            preview.on
              ? t({ id: 'library.preview.stop', message: `Stop ${title}`, comment: '{title} is a sound or a song’s name' })
              : t({ id: 'library.preview.play', message: `Play ${title}`, comment: '{title} is a sound or a song’s name' })
          }
          title={
            preview.on
              ? t({ id: 'library.preview.stop.tip', message: 'Stop' })
              : t({ id: 'library.preview.play.tip', message: 'Play' })
          }
          onClick={preview.toggle}
        >
          <Icon name={TILE_ICON[art]} className="preview-art" />
          <Icon name={preview.on ? 'stop' : 'play'} className="preview-icon" />
        </button>
      ) : (
        <span className={art === 'clip' ? 'art-clip' : `art-tile is-${art}`} aria-hidden>
          <Icon name={TILE_ICON[art]} />
        </span>
      )}
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
      {preview?.sounding && (
        <span
          className="preview-progress"
          aria-hidden
          style={{ animationDuration: `${Math.min(asset.durationUs, PREVIEW_MAX_US) / 1000}ms` }}
        />
      )}
    </li>
  );
}, sameRow);
