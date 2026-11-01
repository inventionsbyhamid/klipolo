import type { ReactNode } from 'react';
import { cn } from '../../../design/cn';
import { Tooltip } from '../../../design/Tooltip';
import { Icon, type IconName } from '../../../layout/Icon';
import { useDropIndicator } from '../dropTarget';
import { HEADER_PX } from '../layout';

/** What a dragged file over a lane would land as: the Video lane takes clips, Overlays pictures, the others sounds. */
const DROPS: Record<string, 'main' | 'picture' | 'sound'> = {
  main: 'main',
  layers: 'picture',
  sounds: 'sound',
  music: 'sound',
};

interface LaneRowProps {
  /**
   * The lane's id as the keyboard walks it (`keys/laneOrder`): layers, main, sounds or music; a recorded sound's lane
   * is `source:<kind>`, which the keyboard and drops pass over.
   */
  lane: string;
  /** The lane's icon in the gutter, and its name (the icon's tooltip, and the lane's name for screen readers). */
  icon?: IconName;
  name: string;
  /** What the gutter holds instead of the icon: a recorded sound's lane has its own controls there. */
  header?: ReactNode;
  /** Nothing on it yet: a faint strip. */
  empty: boolean;
  /** A fixed height (px), or `grow`: the Video lane takes the timeline's spare height. */
  height?: number;
  grow?: boolean;
  width: number;
  onSeek: (px: number) => void;
  children: ReactNode;
}

/**
 * One lane of the timeline: its icon in the gutter, which stays put while the lanes scroll, then the lane. Click empty
 * lane to seek. While a file is dragged over it, it shows a dashed paper edge.
 */
export function LaneRow({
  lane,
  icon,
  name,
  header,
  empty,
  height,
  grow = false,
  width,
  onSeek,
  children,
}: LaneRowProps) {
  // The lane the file is held over, only: both audio lanes take a sound, so a sound's target names its lane (`row`).
  const dropOver = useDropIndicator(
    (s) =>
      s.target !== null &&
      s.target.lane === DROPS[lane] &&
      (s.target.lane !== 'sound' || (s.target.row ?? 'sounds') === lane),
  );
  return (
    <div
      role="group"
      aria-label={name}
      className={cn('tl-row', grow && 'is-grow')}
      style={grow ? undefined : { height }}
      data-lane={lane}
    >
      <div className="tl-gutter" style={{ width: HEADER_PX }}>
        {header ??
          (icon && (
            <Tooltip label={name} side="right">
              <span className="tl-ico" tabIndex={-1}>
                <Icon name={icon} />
              </span>
            </Tooltip>
          ))}
      </div>
      <div
        className={cn('tl-lane', empty && 'is-empty', dropOver && 'is-drop')}
        style={{ width }}
        onClick={(e) => onSeek(e.clientX - e.currentTarget.getBoundingClientRect().left)}
      >
        {children}
      </div>
    </div>
  );
}
