import { t } from '@lingui/core/macro';
import type { Clip } from '../../bindings/Clip';
import type { ContribId } from '../../bindings/ContribId';
import type { Op } from '../../bindings/Op';
import type { Project } from '../../bindings/Project';
import { builtinId, contribLabel } from '../../lib/contrib';
import type { Label } from '../../lib/labels';
import { clipOp } from '../../lib/ops';
import { findClip } from '../../lib/timeline';
import { clipNow, commitLatest } from '../../state/latest';

export type LookChoice = {
  /** A stable key for lists ("none", "warm", …). */
  key: string;
  /** The look contribution, or null for no look. */
  preset: ContribId | null;
  /**
   * The swatch's two colours, top left to bottom right: a picture of what the look does to colour. They are the look's
   * own, not the interface's, so they stay the same in every theme.
   */
  swatch: [string, string];
};

function builtinLook(word: string, swatch: [string, string]): LookChoice {
  return { key: word, preset: builtinId(`look.${word}`), swatch };
}

/** No look, then the built-in looks, in the order the Looks view shows them. */
export const LOOKS: LookChoice[] = [
  { key: 'none', preset: null, swatch: ['#71717a', '#d4d4d8'] },
  builtinLook('warm', ['#d97706', '#fdba74']),
  builtinLook('cool', ['#0369a1', '#67e8f9']),
  builtinLook('mono', ['#262626', '#e5e5e5']),
  builtinLook('vintage', ['#854d0e', '#fecdd3']),
  builtinLook('vivid', ['#c026d3', '#bef264']),
  builtinLook('cinema', ['#115e59', '#fdba74']),
  builtinLook('romcom', ['#ec4899', '#fde68a']),
];

/** A look's name in the language shown: "None", or the look's own name ("Missing look" for one not installed). */
export function presetName(preset: ContribId | null): string {
  return preset ? contribLabel(preset, 'look') : t({ id: 'looks.none', message: 'None', comment: 'No colour look' });
}

/** The undo label for applying a look ("Apply Warm look", by the look's id), or for taking it off ("Remove look"). */
export function applyLookLabel(look: LookChoice): Label {
  return look.preset ? { id: 'looks.apply.undo', params: { look: look.preset } } : 'looks.clear.undo';
}

/** The clips a look applies to: the selected clip, or every clip when no clip is selected. */
export function lookTargets(project: Project, selectedId: string | null): Clip[] {
  const selectedClip = selectedId ? findClip(project, selectedId) : null;
  return selectedClip ? [selectedClip] : project.timeline.main;
}

/** The look every target shares (null: no look), or undefined when they differ or there are none. */
export function sharedPreset(clips: Clip[]): ContribId | null | undefined {
  if (clips.length === 0) return undefined;
  const first = clips[0].look.preset;
  return clips.every((clip) => clip.look.preset === first) ? first : undefined;
}

/** `preset` on each target's look as the project has it when the ops are built (a clip gone by then is left out). */
export function lookOps(targets: Clip[], preset: ContribId | null): Op[] {
  return targets.flatMap((target): Op[] => {
    const clip = clipNow(target.id);
    return clip ? [clipOp(clip, { look: { ...clip.look, preset } })] : [];
  });
}

/** `look` on `targets` as one undo step, entering the commit queue at once and built at its turn. */
export const applyLook = (targets: Clip[], look: LookChoice) =>
  commitLatest(applyLookLabel(look), () => lookOps(targets, look.preset));
