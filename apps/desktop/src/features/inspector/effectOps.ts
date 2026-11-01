import { t } from '@lingui/core/macro';
import type { ContribId } from '../../bindings/ContribId';
import type { EffectInstance } from '../../bindings/EffectInstance';
import type { EffectPatch } from '../../bindings/EffectPatch';
import type { EffectTarget } from '../../bindings/EffectTarget';
import type { Op } from '../../bindings/Op';
import type { ParamValue } from '../../bindings/ParamValue';
import type { Project } from '../../bindings/Project';
import { contribLabel } from '../../lib/contrib';
import type { Label } from '../../lib/labels';
import { findClip, findItem } from '../../lib/timeline';
import { commitLatest, effectsNow } from '../../state/latest';

/** The model's limit (`validate.rs`, `MAX_EFFECTS`). */
export const MAX_EFFECTS = 8;

/** A clip's or layer's effect stack, and how ops name it. */
export interface Stack {
  target: EffectTarget;
  effects: EffectInstance[];
}

/** The stack of block `id`: a clip's, or a layer's that draws (sound and zoom layers have none). */
export function stackOf(project: Project, id: string): Stack | null {
  const found = findItem(project, id);
  if (found) {
    const kind = found.item.content.type;
    return kind === 'audio' || kind === 'zoom' ? null : { target: { type: 'item', id }, effects: found.item.effects };
  }
  const clip = findClip(project, id);
  return clip ? { target: { type: 'clip', id }, effects: clip.effects } : null;
}

/** A new effect at the end of the stack, with any settings given; the engine gives it its id. */
export const addEffectOp = (target: EffectTarget, effect: ContribId, params: Record<string, ParamValue> = {}): Op => ({
  type: 'addEffect',
  target,
  effect: { id: '', effect, enabled: true, params },
  index: null,
});
export const updateEffectOp = (target: EffectTarget, id: string, patch: EffectPatch): Op => ({
  type: 'updateEffect',
  target,
  id,
  patch,
});
export const removeEffectOp = (target: EffectTarget, id: string): Op => ({ type: 'removeEffect', target, id });
export const moveEffectOp = (target: EffectTarget, id: string, index: number): Op => ({
  type: 'moveEffect',
  target,
  id,
  index,
});

/** An effect's name ("Blur"; "Missing effect" when this build lacks it). */
export const effectName = (effect: ContribId) => contribLabel(effect, 'effect');

/**
 * The effect actions' undo labels: message ids with the effect's contribution id as a param, so history never holds
 * its English name; it is worded as the effect's name in the language shown (`contribNames`). A generator's settings
 * use `change` too.
 */
export const effectLabels = {
  add: (effect: ContribId): Label => ({ id: 'effect.add.undo', params: { effect } }),
  change: (effect: ContribId): Label => ({ id: 'effect.change.undo', params: { effect } }),
  turn: (effect: ContribId, on: boolean): Label => ({ id: on ? 'effect.on.undo' : 'effect.off.undo', params: { effect } }),
  move: (effect: ContribId, up: boolean): Label => ({ id: up ? 'effect.up.undo' : 'effect.down.undo', params: { effect } }),
  remove: (effect: ContribId): Label => ({ id: 'effect.remove.undo', params: { effect } }),
};

/** Why an effect command can't act: said as a toast. */
export const effectRefusal = {
  noStack: () =>
    t({ id: 'effect.refusal.noStack', message: 'Select a clip, or a layer that draws, to change its effects.' }),
  noEffect: () => t({ id: 'effect.refusal.noEffect', message: "That effect isn't on this clip or layer." }),
  unknown: () => t({ id: 'effect.refusal.unknown', message: "That effect isn't in this version of Klipolo." }),
  full: () => t({ id: 'effect.refusal.full', message: 'A clip or layer can have up to eight effects.' }),
};

/*
 * The effect actions: the Effects popover and the effect commands (`useEffectCommands`) run these, each one undo step
 * through the commit queue. Their ops are built when they go in, from the stack as the project has it then
 * (`commitLatest`): a quick second action (a move after a move) builds on the first. A settings slider commits through
 * `ToolSlider`'s burst instead, with the same ops (`updateEffectOp`).
 */

/** The ops `build` makes from the stack now, while effect `id` is still in it; none once it's gone. */
const withEffect = (stack: Stack, id: string, build: (effects: EffectInstance[]) => Op[]) => () => {
  const effects = effectsNow(stack.target);
  return effects?.some((e) => e.id === id) ? build(effects) : [];
};

/** `effect` at the end of the stack, with any settings given ("make the facecam round" in one step). */
export const addEffect = (stack: Stack, effect: ContribId, params: Record<string, ParamValue> = {}) =>
  commitLatest(effectLabels.add(effect), () => {
    const effects = effectsNow(stack.target);
    return effects && effects.length < MAX_EFFECTS ? [addEffectOp(stack.target, effect, params)] : [];
  });

export const turnEffect = (stack: Stack, fx: EffectInstance, on: boolean) =>
  commitLatest(
    effectLabels.turn(fx.effect, on),
    withEffect(stack, fx.id, () => [updateEffectOp(stack.target, fx.id, { enabled: on })]),
  );

/** One place up (towards the start, which runs first) or down; nothing at the end it would leave. */
export const moveEffect = (stack: Stack, fx: EffectInstance, up: boolean) =>
  commitLatest(
    effectLabels.move(fx.effect, up),
    withEffect(stack, fx.id, (effects) => {
      const index = effects.findIndex((e) => e.id === fx.id) + (up ? -1 : 1);
      return index < 0 || index >= effects.length ? [] : [moveEffectOp(stack.target, fx.id, index)];
    }),
  );

/** Resolves with whether it took the effect off: false when refused, or when it was already gone (nothing sent). */
export function removeEffect(stack: Stack, fx: EffectInstance): Promise<boolean> {
  let sent = false;
  return commitLatest(
    effectLabels.remove(fx.effect),
    withEffect(stack, fx.id, () => {
      sent = true;
      return [removeEffectOp(stack.target, fx.id)];
    }),
  ).then((ok) => ok && sent);
}

/** Settings to set (`params`) and to take back to their defaults (`reset`), as one change. */
export function setEffectParams(
  stack: Stack,
  fx: EffectInstance,
  params: Record<string, ParamValue>,
  reset: string[] = [],
): Promise<boolean> {
  const patch: EffectPatch = {};
  if (Object.keys(params).length > 0) patch.params = params;
  if (reset.length > 0) patch.reset = reset;
  if (!patch.params && !patch.reset) return Promise.resolve(true);
  return commitLatest(
    effectLabels.change(fx.effect),
    withEffect(stack, fx.id, () => [updateEffectOp(stack.target, fx.id, patch)]),
  );
}
