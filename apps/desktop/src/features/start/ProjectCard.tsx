import { useLingui } from '@lingui/react/macro';
import { useId, type CSSProperties } from 'react';
import type { ProjectRow } from '../../bindings/ProjectRow';
import type { ThumbTile } from '../../bindings/ThumbTile';
import { mediaUrl } from '../../bridge/commands';
import { cn } from '../../design/cn';
import { useExport } from '../../state/export';
import { Menu, MenuContent, MenuItem, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import { openedLabel } from '../../lib/relativeTime';
import { formatDuration } from '../../lib/time';

/** The card's picture is 16:9. */
const PICTURE_ASPECT = 16 / 9;

/**
 * Tile 0 of a thumbnail sprite (or a poster: a sheet of one), whole and centred in the 16:9 picture: as wide as the
 * picture for a wide tile, as tall for a tall one. The sheet is drawn `cols` times the tile's width from its top left,
 * so the tile fills the box.
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

type Unfinished = 'running' | 'paused' | 'unfinished';

/** What a project's unfinished exports amount to, for its card; null when it has none. */
function unfinishedOf(jobs: ReturnType<typeof useExport.getState>['jobs'], projectId: string): Unfinished | null {
  const mine = jobs.filter((j) => j.projectId === projectId);
  if (mine.length === 0) return null;
  if (mine.some((j) => j.state === 'running')) return 'running';
  if (mine.some((j) => j.state === 'paused')) return 'paused';
  return 'unfinished';
}

const UNFINISHED_TEXT: Record<Unfinished, string> = {
  running: 'Exporting',
  paused: 'Export paused',
  unfinished: 'Export unfinished',
};

interface ProjectCardProps {
  row: ProjectRow;
  /** Now, for "Opened 2 hours ago". */
  nowMs: number;
  /** It is opening: the press took. */
  busy: boolean;
  onOpen: (path: string) => void;
  /** Makes a copy of the project and opens it (the card's menu). */
  onDuplicate: (path: string) => void;
}

/**
 * The card's menu, from its More button over the picture (shown on hover and focus): Duplicate, a copy of the project
 * with the same footage and edits, opened as "<name> copy", to make another video from it.
 */
function CardMenu({ row, onDuplicate }: Pick<ProjectCardProps, 'row' | 'onDuplicate'>) {
  const { t } = useLingui();
  const { name, path } = row;
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          className="icon-btn start-more"
          aria-label={t({ id: 'start.card.more', message: `More for ${name}`, comment: '{name} is a project name' })}
        >
          <Icon name="more" />
        </button>
      </MenuTrigger>
      <MenuContent align="end">
        <MenuItem onSelect={() => onDuplicate(path)}>
          {t({ id: 'start.card.duplicate', message: 'Duplicate', comment: 'Project card menu item: a copy, opened' })}
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}

/**
 * One project on the start screen: its first clip's picture (else the clip art), its name and "Opened
 * 2 hours ago, 4:12" (just the length when the time it was opened isn't known), and a quiet badge when it has an
 * unfinished export (after a quit, a crash or a pause), so it is seen from here too: opening the project shows the dot
 * on Export and the rows to resume. A click or Enter opens it. Named by its name and described by that line and the
 * badge. Its menu (`CardMenu`) duplicates it.
 */
export function ProjectCard({ row, nowMs, busy, onOpen, onDuplicate }: ProjectCardProps) {
  const { t } = useLingui();
  const nameId = useId();
  const metaId = useId();
  const badgeId = useId();
  const unfinished = useExport((s) => unfinishedOf(s.jobs, row.id));
  const length = formatDuration(row.durationUs);
  const opened = row.openedMs > 0 ? openedLabel(row.openedMs, nowMs) : null;
  return (
    <li className="start-item">
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        className={cn('start-card', busy && 'is-busy')}
        aria-labelledby={nameId}
        aria-describedby={unfinished ? `${metaId} ${badgeId}` : metaId}
        aria-busy={busy || undefined}
        onClick={() => onOpen(row.path)}
      >
        <span className="start-pic" aria-hidden>
          {/* The clip art is under the tile: a picture that doesn't load leaves it showing. */}
          <Icon name="film" />
          {row.thumb && <span className="start-tile" style={tileStyle(row.thumb)} />}
          {busy && (
            <span className="start-busy">
              <span className="spinner" />
            </span>
          )}
        </span>
        <span className="start-name" id={nameId} title={row.name}>
          {row.name}
        </span>
        <span className="start-meta" id={metaId}>
          {opened === null
            ? length
            : t({
                id: 'start.card.meta',
                message: `${opened}, ${length}`,
                comment: '{opened} is when it was last opened ("Opened 2 hours ago"), {length} its length ("4:12")',
              })}
        </span>
        {unfinished && (
          <span className="start-badge" id={badgeId}>
            {UNFINISHED_TEXT[unfinished]}
          </span>
        )}
      </button>
      <CardMenu row={row} onDuplicate={onDuplicate} />
    </li>
  );
}
