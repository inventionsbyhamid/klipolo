import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { Project } from '../../bindings/Project';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { transitionClipId, useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';
import { useTimelineFocus } from '../../state/timelineFocus';
import { searchWords, type Command, type RunOptions } from '../command-palette/command';
import { cutClipId } from './keys/laneOrder';
import { blockFocused } from './keys/timelineActions';
import { addTransition, clipBeforeNearestCut, transitionLabels } from './keys/transitionActions';
import { mainIndex } from './model/index';

/** Clip `id`'s index when it has a clip after it (a cut follows it), else -1. */
function beforeCut(project: Project, id: string | null): number {
  const main = project.timeline.main;
  const i = id === null ? -1 : main.findIndex((c) => c.id === id);
  return i + 1 < main.length ? i : -1;
}

/**
 * The cut Add transition acts on (the clip before it, by index), chosen without the playhead: the block it was run on
 * (a clip's menu), a cut with focus (`focused`: its clip, read when the command runs), the selected transition, or
 * the selected clip. -1: none of those.
 */
function chosenCut(
  project: Project,
  selectedId: string | null,
  options?: RunOptions,
  focused: string | null = null,
): number {
  if (options?.block !== undefined) return beforeCut(project, options.block);
  if (focused !== null) return beforeCut(project, focused);
  return beforeCut(project, transitionClipId(selectedId) ?? selectedId);
}

/** The cut with focus, when the keyboard is on one: its clip. */
const focusedCut = () => (blockFocused() ? cutClipId(useTimelineFocus.getState().activeId) : null);

/**
 * Add transition (`useTimelineCommands` merges it), ⌘T and a clip's menu: a half-second dissolve, or the
 * longest that fits, committed through the same action as a cut's + (`addTransition`). It acts on the cut after
 * `options.block` (a clip id, from the clip's menu), else a focused cut, else the selected transition or clip, else
 * the cut nearest the playhead. Removing one is Delete's (`useCommands`); changing one is the transition card's.
 */
export function useTransitionCommands(): Command[] {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  return useMemo(() => {
    if (!project) return [];
    return [
      {
        id: 'add-transition',
        action: 'setTransition',
        group: 'edit',
        label: transitionLabels.add(),
        shortcut: '⌘T',
        keywords: searchWords(
          t({
            id: 'command.addTransition.keywords',
            message: 'dissolve, fade between',
            comment: 'Comma-separated search words for Add transition',
          }),
        ),
        // Whenever there's a clip: a cut with no room says so (`addTransition`'s toast), and so does a video of one
        // clip, which has no cut; never nothing.
        enabled: project.timeline.main.length > 0,
        run: (options) => {
          const now = useProject.getState().snapshot?.project;
          if (!now) return;
          if (now.timeline.main.length < 2) return toast.info(transitionLabels.needsTwo());
          let i = chosenCut(now, useSelection.getState().selectedId, options, focusedCut());
          if (i < 0 && options?.block === undefined)
            i = clipBeforeNearestCut(mainIndex(now.timeline.main), usePlayback.getState().playheadUs);
          if (i < 0)
            return toast.info(t({ id: 'transition.needsNext', message: 'A transition needs a clip after this one.' }));
          addTransition(now.timeline.main[i].id);
        },
      },
    ];
  }, [t, project]);
}
