import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { EffectInstance } from '../../bindings/EffectInstance';
import { BUILTINS } from '../../lib/contrib';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';
import type { Command, FocusedRow, RunOptions } from '../command-palette/command';
import {
  addEffect,
  effectRefusal as refusal,
  MAX_EFFECTS,
  moveEffect,
  removeEffect,
  setEffectParams,
  stackOf,
  turnEffect,
  type Stack,
} from './effectOps';

/**
 * The focused effect row's effect in its stack now: where it is, and whether this build has it (the row's menu offers
 * only Remove for one it lacks, and no Move up for the first or Move down for the last).
 */
function rowEffect(row: FocusedRow): { index: number; last: number; installed: boolean } | null {
  const project = useProject.getState().snapshot?.project;
  const id = row.block ?? useSelection.getState().selectedId;
  const found = project && id ? stackOf(project, id) : null;
  const index = found ? found.effects.findIndex((e) => e.id === row.effectId) : -1;
  if (!found || index < 0) return null;
  const effect = found.effects[index].effect;
  const installed = BUILTINS.some((d) => d.id === effect && d.kind === 'effect');
  return { index, last: found.effects.length - 1, installed };
}

/**
 * The effect commands: add, change, switch, move and remove an effect on a clip or a layer, each one undo step running
 * the same action as the Effects popover (`effectOps`). Each names the block it acts on (`options.block`, else the
 * selection) and, but for `effect-add`, the effect by its id in that stack (`options.effectId`). A list of commands
 * shows them only while an effect row has focus (`needs: 'effect'`), when they can act on its effect as the row's menu
 * decides (`fits`); it has no settings to give `effect-set` and no effect type to give `effect-add`, so it leaves those
 * two out. `enabled` says whether the selection can take them. An effect added with settings is one step:
 * `effect-add { block, effect: 'klipolo.builtin/effect.mask', params: { shape: 'circle' } }`.
 */
export function useEffectCommands(): Command[] {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  return useMemo(() => {
    const selected = project && selectedId ? stackOf(project, selectedId) : null;
    const canAdd = selected !== null && selected.effects.length < MAX_EFFECTS;
    const hasEffects = (selected?.effects.length ?? 0) > 0;
    /** The stack the run names; a toast says why when there's none. */
    const stack = (options?: RunOptions): Stack | null => {
      const project = useProject.getState().snapshot?.project;
      const id = options?.block ?? useSelection.getState().selectedId;
      const found = project && id ? stackOf(project, id) : null;
      if (!found) toast.info(refusal.noStack());
      return found;
    };
    /** The run's stack and its effect `options.effectId`. */
    const on = (run: (stack: Stack, fx: EffectInstance, options: RunOptions) => void) => (options?: RunOptions) => {
      const found = stack(options);
      if (!found) return;
      const fx = found.effects.find((e) => e.id === options?.effectId);
      if (fx) run(found, fx, options ?? {});
      else toast.info(refusal.noEffect());
    };
    return [
      {
        id: 'effect-add',
        action: 'addEffect',
        group: 'edit',
        label: t({ id: 'effect.add', message: 'Add effect', comment: 'Button and command' }),
        enabled: canAdd,
        needs: 'effect',
        paletteHidden: true,
        // `params`: settings it starts with.
        run: (options) => {
          const effect = options?.effect;
          if (!effect || !BUILTINS.some((d) => d.id === effect && d.kind === 'effect'))
            return toast.info(refusal.unknown());
          const found = stack(options);
          if (!found) return;
          if (found.effects.length >= MAX_EFFECTS) return toast.info(refusal.full());
          void addEffect(found, effect, options?.params);
        },
      },
      {
        id: 'effect-set',
        action: 'updateEffect',
        group: 'edit',
        label: t({ id: 'effect.set', message: 'Change effect settings', comment: 'Command' }),
        enabled: hasEffects,
        needs: 'effect',
        paletteHidden: true,
        // `params` to set, `reset` keys to take back to their defaults.
        run: on((found, fx, options) => void setEffectParams(found, fx, options.params ?? {}, options.reset)),
      },
      {
        id: 'effect-toggle',
        action: 'updateEffect',
        group: 'edit',
        label: t({ id: 'effect.toggle', message: 'Turn effect on or off', comment: 'Command' }),
        enabled: hasEffects,
        needs: 'effect',
        fits: (row) => rowEffect(row)?.installed === true,
        // `enabled` is the state to end in (an effect's switch); without one it switches to the other state.
        run: on((found, fx, options) => {
          const enabled = options.enabled ?? !fx.enabled;
          if (enabled !== fx.enabled) void turnEffect(found, fx, enabled);
        }),
      },
      {
        id: 'effect-move-up',
        action: 'moveEffect',
        group: 'edit',
        label: t({ id: 'effect.moveUp', message: 'Move effect up', comment: 'Command' }),
        enabled: hasEffects,
        needs: 'effect',
        fits: (row) => {
          const at = rowEffect(row);
          return !!at?.installed && at.index > 0;
        },
        run: on((found, fx) => void moveEffect(found, fx, true)),
      },
      {
        id: 'effect-move-down',
        action: 'moveEffect',
        group: 'edit',
        label: t({ id: 'effect.moveDown', message: 'Move effect down', comment: 'Command' }),
        enabled: hasEffects,
        needs: 'effect',
        fits: (row) => {
          const at = rowEffect(row);
          return !!at?.installed && at.index < at.last;
        },
        run: on((found, fx) => void moveEffect(found, fx, false)),
      },
      {
        id: 'effect-remove',
        action: 'removeEffect',
        group: 'edit',
        label: t({ id: 'effect.removeCommand', message: 'Remove effect', comment: 'Command' }),
        enabled: hasEffects,
        needs: 'effect',
        fits: (row) => rowEffect(row) !== null,
        run: on((found, fx) => void removeEffect(found, fx)),
      },
    ];
  }, [t, project, selectedId]);
}
