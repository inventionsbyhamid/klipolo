import { useLingui } from '@lingui/react/macro';
import type { CSSProperties } from 'react';
import type { ProjectRow } from '../../bindings/ProjectRow';
import type { ThumbTile } from '../../bindings/ThumbTile';
import { mediaUrl } from '../../bridge/commands';
import { Icon } from '../../layout/Icon';
import { openedLabel } from '../../lib/relativeTime';
import { formatDuration } from '../../lib/time';

/** The card's picture is 16:9. */
const PICTURE_ASPECT = 16 / 9;

/**
 * Tile 0 of a thumbnail sprite, whole and centred in the 16:9 picture: as wide as the picture for a wide tile, as tall
 * for a tall one. The sheet is drawn `cols` times the tile's width from its top left, so the tile fills the box.
 */
function tileStyle(thumb: ThumbTile): CSSProperties {
  const aspect = thumb.tileW / thumb.tileH;
  const wide = aspect >= PICTURE_ASPECT;
  return {
    width: wide ? '100%' : `${(aspect / PICTURE_ASPECT) * 100}%`,
    height: wide ? `${(PICTURE_ASPECT / aspect) * 100}%` : '100%',
    backgroundImage: `url("${mediaUrl(thumb.sprite)}")`,
    backgroundSize: `${thumb.cols * 100}% auto`,
    backgroundPosition: '0px 0px',
  };
}

interface ProjectCardProps {
  row: ProjectRow;
  /** Now, for "Opened 2 hours ago". */
  nowMs: number;
  onOpen: (path: string) => void;
}

/**
 * One project on the start screen: its first clip's picture (else the clip art), its name and "Opened
 * 2 hours ago, 4:12". A click or Enter opens it.
 */
export function ProjectCard({ row, nowMs, onOpen }: ProjectCardProps) {
  const { t } = useLingui();
  const opened = openedLabel(row.openedMs, nowMs);
  const length = formatDuration(row.durationUs);
  return (
    <li>
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        className="start-card"
        onClick={() => onOpen(row.path)}
      >
        <span className="start-pic" aria-hidden>
          {row.thumb ? <span className="start-tile" style={tileStyle(row.thumb)} /> : <Icon name="film" />}
        </span>
        <span className="start-name" title={row.name}>
          {row.name}
        </span>
        <span className="start-meta">
          {t({
            id: 'start.card.meta',
            message: `${opened}, ${length}`,
            comment: '{opened} is when it was last opened ("Opened 2 hours ago"), {length} its length ("4:12")',
          })}
        </span>
      </button>
    </li>
  );
}
