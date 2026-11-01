import { fireEvent, screen } from '@testing-library/react';
import { vi } from 'vitest';
import type { Clip } from '../bindings/Clip';
import type { EffectInstance } from '../bindings/EffectInstance';
import type { EffectTarget } from '../bindings/EffectTarget';
import type { Item } from '../bindings/Item';
import type { ParamValue } from '../bindings/ParamValue';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import type { Transaction } from '../bindings/Transaction';
import { findItem } from '../lib/timeline';
import { useProject } from '../state/project';
import { fixtureSnapshot, titleItem } from './fixtures';

/*
 * The effect stacks' and layer cards' tests: a stand-in engine that applies their ops to the store, and the stores they
 * start from. Import it after `vi.mock('../../bridge/commands')` (hoisted), like the modules under test.
 */

let made = 0;

/**
 * A stand-in for the engine's `project_apply`: applies layer and effect ops (`updateItem`, `updateClip`, `addEffect`,
 * `updateEffect`, `removeEffect`, `moveEffect`) to the store's project and answers with the patch, after `ms` (a commit
 * still on its way while another control is used) or in the next microtask. Use it as
 * `vi.mocked(bridge.apply).mockImplementation(engineApply(50))`.
 */
export function engineApply(ms = 0) {
  return (tx: Transaction): Promise<SnapshotPatch> =>
    new Promise((resolve) => {
      const land = () => {
        const s = useProject.getState().snapshot!;
        const clips = new Map<string, Clip>();
        const items = new Map<string, Item>();
        const clip = (id: string) => clips.get(id) ?? s.project.timeline.main.find((c) => c.id === id)!;
        const item = (id: string) => items.get(id) ?? findItem(s.project, id)!.item;
        const edit = (target: EffectTarget, change: (stack: EffectInstance[]) => EffectInstance[]) => {
          if (target.type === 'clip') clips.set(target.id, { ...clip(target.id), effects: change(clip(target.id).effects) });
          else items.set(target.id, { ...item(target.id), effects: change(item(target.id).effects) });
        };
        for (const op of tx.ops) {
          if (op.type === 'updateItem') items.set(op.id, { ...item(op.id), ...op.patch });
          else if (op.type === 'updateClip') clips.set(op.id, { ...clip(op.id), ...op.patch } as Clip);
          else if (op.type === 'addEffect')
            edit(op.target, (stack) => [...stack, { ...op.effect, id: op.effect.id || `new-${++made}` }]);
          else if (op.type === 'removeEffect') edit(op.target, (stack) => stack.filter((e) => e.id !== op.id));
          else if (op.type === 'moveEffect')
            edit(op.target, (stack) => {
              const rest = stack.filter((e) => e.id !== op.id);
              rest.splice(op.index, 0, stack.find((e) => e.id === op.id)!);
              return rest;
            });
          else if (op.type === 'updateEffect')
            edit(op.target, (stack) =>
              stack.map((e) => {
                if (e.id !== op.id) return e;
                const params = { ...e.params };
                for (const key of op.patch.reset ?? []) delete params[key];
                return { ...e, enabled: op.patch.enabled ?? e.enabled, params: { ...params, ...op.patch.params } };
              }),
            );
        }
        resolve({
          baseRevision: s.history.revision,
          history: { ...s.history, revision: s.history.revision + 1 },
          patch: { clips: [...clips.values()], items: [...items.values()] },
        });
      };
      if (ms > 0) setTimeout(land, ms);
      else queueMicrotask(land);
    });
}

/**
 * A pointer press and release at `clientX` on the slider called `name`. jsdom has no layout or pointer capture: its
 * track is 100 px wide and keeps the pointer that pressed it. Radix wraps the thumb in a positioning span in the root.
 */
export function releaseSlider(name: string, clientX: number) {
  const root = screen.getByRole('slider', { name }).parentElement!.parentElement!;
  root.getBoundingClientRect = () => ({ left: 0, width: 100, top: 0, height: 20 }) as DOMRect;
  const captured = vi.spyOn(Element.prototype, 'hasPointerCapture').mockReturnValue(true);
  Object.assign(Element.prototype, { setPointerCapture: () => undefined, releasePointerCapture: () => undefined });
  fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX });
  fireEvent.pointerUp(root, { pointerId: 1, clientX });
  captured.mockRestore();
}

/** An effect of the stack: `effect` is a built-in's local name (`blur`). */
export const fx = (id: string, effect: string, params: Record<string, ParamValue> = {}): EffectInstance => ({
  id,
  effect: `klipolo.builtin/effect.${effect}`,
  enabled: true,
  params,
});

/** The fixture with extra layers on the overlay track (after the title, so they never overlap it). */
export function withLayers(...items: Partial<Item>[]) {
  const snapshot = fixtureSnapshot();
  items.forEach((item, i) =>
    snapshot.project.timeline.tracks[0].items.push({
      ...titleItem,
      attach: null,
      startUs: (5 + i * 5) * 1_000_000,
      ...item,
    } as Item),
  );
  useProject.setState({ snapshot, pending: 0 });
  return snapshot;
}

/** Puts `effects` on clip "a" in the store. */
export function onClipA(effects: EffectInstance[]) {
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.main[0].effects = effects;
  useProject.setState({ snapshot, pending: 0 });
  return snapshot;
}

