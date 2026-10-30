import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { Project } from '../../bindings/Project';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { useUi } from '../../state/ui';
import { searchWords, type Command } from '../command-palette/command';
import { focusIdOf, LANE_KEYS, laneOrder, placeOf, type LaneKey } from './keys/laneOrder';
import { moveFocusTo, seekEdit, selectionStep } from './keys/timelineActions';
import { pickBlock } from './drag/blockPress';
import { videoDurationUs } from '../../lib/timeline';
import { fitTimeline, zoomTimeline } from './zoomControl';

/** Whether selection `id` is one the timeline draws (a clip or a layer). */
const laneHas = (project: Project, id: string, expanded: boolean) =>
  placeOf(laneOrder(project, expanded), focusIdOf(id)) !== null;

/**
 * The timeline's commands that move around rather than edit (`useTimelineCommands` merges them): the selection from
 * block to block, the playhead between edits and to either end, zoom and snapping. Stable ids.
 */
export function useTimelineNavCommands(): Command[] {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  const snapping = useUi((s) => s.snapping);
  const expanded = useUi((s) => s.timelineExpanded);
  const durationUs = project ? videoDurationUs(project.timeline) : 0;
  // The playhead changes every frame: these re-render only when they flip.
  const atStart = usePlayback((s) => s.playheadUs <= 0);
  const atEnd = usePlayback((s) => s.playheadUs >= durationUs);
  return useMemo(() => {
    if (!project) return [];

    /** The block `key` takes the selection to, from the selected one. */
    const select = (id: string, key: LaneKey, label: string): Command => ({
      id,
      action: 'select',
      group: 'edit',
      label,
      enabled: selectedId !== null && selectionStep(project, selectedId, key) !== null,
      run: () => {
        const now = useProject.getState().snapshot?.project;
        const next = now && selectedId ? selectionStep(now, selectedId, key) : null;
        if (now && next) moveFocusTo(now, next);
      },
    });

    return [
      select(
        'select-previous',
        LANE_KEYS.previous,
        t({ id: 'command.selectPrevious', message: 'Select previous' }),
      ),
      select('select-next', LANE_KEYS.next, t({ id: 'command.selectNext', message: 'Select next' })),
      select('select-above', LANE_KEYS.above, t({ id: 'command.selectAbove', message: 'Select above' })),
      select('select-below', LANE_KEYS.below, t({ id: 'command.selectBelow', message: 'Select below' })),
      select(
        'select-first',
        LANE_KEYS.first,
        t({
          id: 'command.selectFirst',
          message: 'Select first',
          comment: 'The first clip or layer in its lane',
        }),
      ),
      select(
        'select-last',
        LANE_KEYS.last,
        t({
          id: 'command.selectLast',
          message: 'Select last',
          comment: 'The last clip or layer in its lane',
        }),
      ),
      {
        id: 'pick',
        action: 'select',
        group: 'playback',
        label: t({
          id: 'command.pick',
          message: 'Go to the selection',
          comment: 'Moves the playhead to the selected clip or layer (Enter on a focused block)',
        }),
        enabled: selectedId !== null && laneHas(project, selectedId, expanded),
        run: () => {
          if (selectedId) pickBlock(selectedId);
        },
      },
      {
        id: 'prev-edit',
        action: 'goTo',
        group: 'playback',
        label: t({
          id: 'command.prevEdit',
          message: 'Previous edit',
          comment: 'Move the playhead to the previous cut or layer edge',
        }),
        shortcut: '↑',
        enabled: !atStart,
        run: () => seekEdit(-1),
      },
      {
        id: 'next-edit',
        action: 'goTo',
        group: 'playback',
        label: t({
          id: 'command.nextEdit',
          message: 'Next edit',
          comment: 'Move the playhead to the next cut or layer edge',
        }),
        shortcut: '↓',
        enabled: durationUs > 0 && !atEnd,
        run: () => seekEdit(1),
      },
      {
        id: 'go-start',
        action: 'goTo',
        group: 'playback',
        label: t({ id: 'command.goStart', message: 'Go to start' }),
        shortcut: t({ id: 'key.home', message: 'Home', comment: 'The Home key, as a keyboard shortcut' }),
        enabled: !atStart,
        run: () => usePlayback.getState().seek(0),
      },
      {
        id: 'go-end',
        action: 'goTo',
        group: 'playback',
        label: t({ id: 'command.goEnd', message: 'Go to end' }),
        shortcut: t({ id: 'key.end', message: 'End', comment: 'The End key, as a keyboard shortcut' }),
        enabled: durationUs > 0 && !atEnd,
        run: () => {
          const now = useProject.getState().snapshot?.project;
          if (now) usePlayback.getState().seek(videoDurationUs(now.timeline));
        },
      },
      {
        id: 'zoom-in',
        action: 'timelineView',
        group: 'view',
        label: t({ id: 'command.zoomIn', message: 'Zoom in timeline' }),
        shortcut: '⌘=',
        enabled: true,
        run: () => zoomTimeline(1.25),
      },
      {
        id: 'zoom-out',
        action: 'timelineView',
        group: 'view',
        label: t({ id: 'command.zoomOut', message: 'Zoom out timeline' }),
        shortcut: '⌘−',
        enabled: true,
        run: () => zoomTimeline(0.8),
      },
      {
        id: 'zoom-fit',
        action: 'timelineView',
        group: 'view',
        label: t({ id: 'command.zoomFit', message: 'Fit the timeline' }),
        shortcut: '⇧Z',
        keywords: searchWords(
          t({
            id: 'command.zoomFit.keywords',
            message: 'zoom to fit, whole video',
            comment: 'Comma-separated search words for Fit the timeline',
          }),
        ),
        enabled: true,
        run: fitTimeline,
      },
      {
        id: 'snapping',
        action: 'viewOptions',
        group: 'view',
        label: snapping
          ? t({ id: 'command.snapping.off', message: 'Turn snapping off' })
          : t({ id: 'command.snapping.on', message: 'Turn snapping on' }),
        shortcut: 'N',
        keywords: searchWords(
          t({
            id: 'command.snapping.keywords',
            message: 'magnet, snap',
            comment: 'Comma-separated search words for turning snapping on or off',
          }),
        ),
        enabled: true,
        run: () => useUi.getState().setSnapping(!snapping),
      },
    ];
  }, [t, project, selectedId, snapping, expanded, durationUs, atStart, atEnd]);
}
