import { useId } from 'react';
import { RecordIcon } from './icons';
import { LivePicture, useStale } from './LivePreview';

export interface ThumbCardProps {
  /** The screen's or the app's name: the card's name. */
  name: string;
  /** "2560 x 1440" for a screen; null when there is nothing to add. */
  meta: string | null;
  /** A still of it (an object URL), null until one arrives: the screen or app icon shows meanwhile. */
  picture: string | null;
  /** When the still came (ms since 1970): one a few seconds old is grey, as its stills have stopped. */
  pictureAt?: number | null;
  checked: boolean;
  /** Which icon stands in for the picture. */
  kind: 'screen' | 'app';
  /** In the group's Tab order: the checked card, or the first while none is. */
  tabbable: boolean;
  /** The card's value, so the group's arrow keys can find it. */
  value: string;
  onPick: () => void;
}

/**
 * One screen or app in "What to record": a raised card with a 16:9 picture, its name and its meta. A radio
 * in its group, named by its name and described by its meta; checked, it has the 2 px paper ring. Its still refreshes
 * about once a second, and turns grey once they stop coming.
 */
export function ThumbCard({
  name,
  meta,
  picture,
  pictureAt = null,
  checked,
  kind,
  tabbable,
  value,
  onPick,
}: ThumbCardProps) {
  const nameId = useId();
  const metaId = useId();
  const stale = useStale(picture === null ? null : pictureAt);
  return (
    <button
      type="button"
      role="radio"
      className="record-card"
      aria-checked={checked}
      aria-labelledby={nameId}
      aria-describedby={meta === null ? undefined : metaId}
      data-value={value}
      tabIndex={tabbable ? 0 : -1}
      onClick={onPick}
    >
      <span className="record-pic" aria-hidden data-stale={stale || undefined}>
        {/* The icon is under the picture: until a still is drawn, it shows. */}
        <RecordIcon name={kind} />
        <LivePicture url={picture} />
      </span>
      <span className="record-name" id={nameId} title={name}>
        {name}
      </span>
      {meta !== null && (
        <span className="record-meta" id={metaId}>
          {meta}
        </span>
      )}
    </button>
  );
}
