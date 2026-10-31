import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { Project } from '../../bindings/Project';
import { BUILTINS } from '../../lib/contrib';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { transitionClipId, useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';
import { useTimelineFocus } from '../../state/timelineFocus';
import { searchWords, type Command, type RunOptions } from '../command-palette/command';
import { cutClipId } from './keys/laneOrder';
import { blockFocused } from './keys/timelineActions';
import {
  addTransition,
  changeTransition,
  nearestCut,
  removeTransition,
  transitionLabels,
} from './keys/transitionActions';
import { mainIndex } from './model/index';

/** Clip `id`'s index when it has a clip after it (a cut follows it), else -1. */
function beforeCut(project: Project, id: string | null): number {
  const main = project.timeline.main;
  const i = id === null ? -1 : main.findIndex((c) => c.id === id);
  return i + 1 < main.length ? i : -1;
}

/**
 * The cut a transition command acts on (the clip before it, by index), chosen without the playhead: the block it was
 * run on (a clip's menu), a cut with focus (`focused`: its clip, read when a command runs), the selected
 * transition, or the selected clip. -1: none of those.
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

/** Whether `effect` names a transition this build has. */
const installed = (effect: string) => BUILTINS.some((d) => d.id === effect && d.kind === 'transition');

/**
 * The transition commands (`useTimelineCommands` merges them): add one (⌘T), change one, remove one. Each
 * commits one `setTransition` through the same actions as the cut markers and the transition card. They act on the
 * cut after `options.block` (a clip id, from the clip's menu), else a focused cut, else the selected transition or
 * clip; Add transition then falls back to the cut nearest the playhead.
 */
export function useTransitionCommands(): Command[] {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  return useMemo(() => {
    if (!project) return [];
    const chosen = chosenCut(project, selectedId);
    const selectedTransition = transitionClipId(selectedId);
    const unknown = () => toast.info(t({ id: 'transition.unknown', message: "That transition isn't installed." }));
    /** The clip whose transition a change or a removal names; a toast says so when there's none. */
    const withTransition = (options?: RunOptions): string | null => {
      const now = useProject.getState().snapshot?.project;
      const i = now ? chosenCut(now, useSelection.getState().selectedId, options, focusedCut()) : -1;
      if (now && i >= 0 && now.timeline.main[i].transitionOut) return now.timeline.main[i].id;
      toast.info(t({ id: 'transition.none', message: "There's no transition there." }));
      return null;
    };
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
        // Whenever there's a cut to act on: one with no room says so (`addTransition`'s toast), never nothing.
        enabled: chosen >= 0 || project.timeline.main.length > 1,
        // `effect` and `durationUs`, when given, name the type and length; else a half-second dissolve.
        run: (options) => {
          const now = useProject.getState().snapshot?.project;
          if (!now) return;
          let i = chosenCut(now, useSelection.getState().selectedId, options, focusedCut());
          if (i < 0 && options?.block === undefined)
            i = nearestCut(mainIndex(now.timeline.main), usePlayback.getState().playheadUs);
          const effect = options?.effect;
          if (effect !== undefined && !installed(effect)) return unknown();
          if (i < 0)
            return toast.info(t({ id: 'transition.needsNext', message: 'A transition needs a clip after this one.' }));
          addTransition(now.timeline.main[i].id, { effect, durationUs: options?.durationUs });
        },
      },
      {
        id: 'transition-set',
        action: 'setTransition',
        group: 'edit',
        label: t({ id: 'transition.set', message: 'Change transition', comment: 'Command' }),
        // The card sets the type, length and settings: there's nothing for a key or menu to give it.
        paletteHidden: true,
        enabled: selectedTransition !== null,
        // `effect`, `durationUs`, `params` and `reset` say what changes.
        run: (options) => {
          const clipId = withTransition(options);
          if (clipId === null) return;
          const effect = options?.effect;
          if (effect !== undefined && !installed(effect)) return unknown();
          changeTransition(clipId, {
            effect,
            durationUs: options?.durationUs,
            params: options?.params,
            reset: options?.reset,
          });
        },
      },
      {
        id: 'transition-remove',
        action: 'setTransition',
        group: 'edit',
        label: transitionLabels.remove(),
        // Delete says "Remove transition" with one selected.
        paletteHidden: true,
        enabled: selectedTransition !== null,
        run: (options) => {
          const clipId = withTransition(options);
          if (clipId !== null) removeTransition(clipId);
        },
      },
    ];
  }, [t, project, selectedId]);
}
