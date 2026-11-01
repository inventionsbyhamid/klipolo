import { useLingui } from '@lingui/react/macro';
import { useId, useState } from 'react';
import type { EffectInstance } from '../../bindings/EffectInstance';
import { IconButton } from '../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../design/Menu';
import { Toggle } from '../../design/Toggle';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import { BUILTINS } from '../../lib/contrib';
import { commitToggle } from '../../state/commit';
import { effectsNow } from '../../state/latest';
import { effectLabels, moveEffect, updateEffectOp, type Stack } from './effectOps';
import { ParamFields } from './ParamFields';

interface EffectRowProps {
  stack: Stack;
  fx: EffectInstance;
  index: number;
  /** Its name in the stack ("Vignette (2)" when there are two). */
  name: string;
  /** The same inside a sentence ("missing effect (2)"), for the labels that name it. */
  sentence: string;
  /** Remove it (the stack moves focus on once it's gone). */
  onRemove: () => void;
}

/**
 * One effect of a stack: a switch that turns it on or off, its name, its settings (generated from its schema) behind
 * "Show … settings", and a menu to move it up or down the stack (which runs top to bottom) or remove it. An effect this
 * build doesn't have shows "Missing effect" (its id as a tooltip) and can only be removed.
 */
export function EffectRow({ stack, fx, index, name, sentence, onRemove }: EffectRowProps) {
  const { t } = useLingui();
  const [open, setOpen] = useState(false);
  const settingsId = useId();
  const def = BUILTINS.find((d) => d.id === fx.effect && d.kind === 'effect');
  const ids = { 'data-effect-row': '', 'data-effect-id': fx.id, 'data-effect-block': stack.target.id };
  if (!def) {
    return (
      <li {...ids} className="fx-row">
        <div className="fx-head">
          <span title={fx.effect} className="fx-name fx-missing">
            {name}
          </span>
          {/* "Remove missing effect", as its undo label says it: this build doesn't know its name. */}
          <IconButton
            className="icon-btn-sm"
            icon={<Icon name="trash" />}
            label={t({ id: 'effect.remove.named', message: `Remove ${sentence}`, comment: '{sentence}: e.g. missing effect (2)' })}
            onClick={onRemove}
          />
        </div>
      </li>
    );
  }
  const last = stack.effects.length - 1;
  /** What the row's switches act on: this effect of this block. */
  const owner = `${stack.target.id}:${fx.id}`;
  return (
    <li {...ids} className="fx-row">
      <div className="fx-head">
        <Tooltip
          label={
            fx.enabled
              ? t({ id: 'effect.turnOff.named', message: `Turn ${sentence} off`, comment: '{sentence}: e.g. Vignette (2)' })
              : t({ id: 'effect.turnOn.named', message: `Turn ${sentence} on`, comment: '{sentence}: e.g. Vignette (2)' })
          }
        >
          <Toggle
            aria-label={name}
            checked={fx.enabled}
            // Each press flips what the project has as it goes in, so two quick presses turn it off and back on.
            onCheckedChange={() =>
              void commitToggle(
                () => effectsNow(stack.target)?.find((e) => e.id === fx.id)?.enabled ?? fx.enabled,
                (on) => effectLabels.turn(fx.effect, on),
                (on) => [updateEffectOp(stack.target, fx.id, { enabled: on })],
                { id: owner },
              )
            }
          />
        </Tooltip>
        <span className="fx-name">{name}</span>
        {def.params.length > 0 && (
          <IconButton
            className="icon-btn-sm"
            icon={<Icon name={open ? 'chev-down' : 'chev-right'} />}
            label={
              open
                ? t({ id: 'effect.settings.hide', message: `Hide ${name} settings`, comment: '{name}: e.g. Blur' })
                : t({ id: 'effect.settings.show', message: `Show ${name} settings`, comment: '{name}: e.g. Blur' })
            }
            aria-expanded={open}
            aria-controls={open ? settingsId : undefined}
            onClick={() => setOpen((o) => !o)}
          />
        )}
        <Menu>
          <MenuTrigger asChild>
            <IconButton
              className="icon-btn-sm"
              icon={<Icon name="more" />}
              label={t({ id: 'effect.options', message: `${name} options`, comment: '{name}: e.g. Blur' })}
            />
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem disabled={index === 0} onSelect={() => void moveEffect(stack, fx, true)}>
              {t({ id: 'effect.up', message: 'Move up', comment: 'Menu item: earlier in the effect stack' })}
            </MenuItem>
            <MenuItem disabled={index === last} onSelect={() => void moveEffect(stack, fx, false)}>
              {t({ id: 'effect.down', message: 'Move down', comment: 'Menu item: later in the effect stack' })}
            </MenuItem>
            <MenuSeparator />
            <MenuItem onSelect={onRemove}>
              {t({ id: 'effect.remove', message: 'Remove', comment: 'Menu item: take the effect off' })}
            </MenuItem>
          </MenuContent>
        </Menu>
      </div>
      {open && (
        <div id={settingsId} className="fx-settings">
          <ParamFields
            def={def}
            values={fx.params}
            opsFor={(key, value) => [updateEffectOp(stack.target, fx.id, { params: { [key]: value } })]}
            resetOps={(key) => [updateEffectOp(stack.target, fx.id, { reset: [key] })]}
            owner={owner}
            valuesNow={() => effectsNow(stack.target)?.find((e) => e.id === fx.id)?.params ?? fx.params}
            commitLabel={effectLabels.change(fx.effect)}
          />
        </div>
      )}
    </li>
  );
}
