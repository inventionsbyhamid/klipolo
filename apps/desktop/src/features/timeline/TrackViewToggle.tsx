import { useLingui } from '@lingui/react/macro';
import { FoldVertical, UnfoldVertical } from 'lucide-react';
import { memo } from 'react';
import { IconButton } from '../../design/IconButton';
import { useUi } from '../../state/ui';

/** "Show all tracks" or "Show fewer tracks", in the language shown: what the track view's switch does next. */
export function useTrackViewLabel(): string {
  const { t } = useLingui();
  const expanded = useUi((s) => s.timelineExpanded);
  return expanded
    ? t({ id: 'timeline.tracks.fewer', message: 'Show fewer tracks' })
    : t({ id: 'timeline.tracks.all', message: 'Show all tracks' });
}

/**
 * The track view's switch, in the corner above the lanes' gutter: Show all tracks (every track in a row of its own,
 * with its header) or Show fewer tracks (the four lanes). Drawn again only as the track view changes.
 */
export const TrackViewToggle = memo(function TrackViewToggle() {
  const expanded = useUi((s) => s.timelineExpanded);
  const label = useTrackViewLabel();
  const Glyph = expanded ? FoldVertical : UnfoldVertical;
  return (
    <IconButton
      className="tl-lane-btn"
      icon={<Glyph size={14} strokeWidth={1.8} aria-hidden />}
      label={label}
      onClick={() => useUi.getState().setTimelineExpanded(!expanded)}
    />
  );
});
