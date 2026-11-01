import { useLingui } from '@lingui/react/macro';
import { useLayoutEffect, useRef, useState, type ButtonHTMLAttributes, type Ref } from 'react';
import type { EffectInstance } from '../../bindings/EffectInstance';
import type { EffectTarget } from '../../bindings/EffectTarget';
import { Menu, MenuContent, MenuItem, MenuTrigger } from '../../design/Menu';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import { builtinsOf, contribLabel } from '../../lib/contrib';
import { addEffect, effectName, MAX_EFFECTS, removeEffect } from './effectOps';
import { EffectRow } from './EffectRow';

interface EffectStackProps {
  target: EffectTarget;
  effects: EffectInstance[];
}

/** The built-in effects, in catalogue order. */
const EFFECTS = builtinsOf('effect');

/**
 * A clip's or layer's effects, top to bottom in the order they run, then "Add effect" (a menu of the effects this
 * build has; inert, saying why, at the model's eight). Every change is one undo step through the commit queue, the
 * path the effect commands take too.
 *
 * Focus never drops to the page, where the next Tab would close the popover: a removed row hands it to the row now in
 * its place (else the one before, else "Add effect"), and "Add effect" stays the same button at eight.
 */
export function EffectStack({ target, effects }: EffectStackProps) {
  const { t } = useLingui();
  const root = useRef<HTMLDivElement>(null);
  const add = useRef<HTMLButtonElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  /** Where a row was removed: its place takes focus once the stack has changed. */
  const removedAt = useRef<number | null>(null);
  const stack = { target, effects };
  const full = effects.length >= MAX_EFFECTS;
  const ids = effects.map((fx) => fx.id).join('\n');

  useLayoutEffect(() => {
    const at = removedAt.current;
    if (at === null) return;
    removedAt.current = null;
    const active = document.activeElement;
    // Focus that went somewhere on purpose (the timeline, another control) stays there.
    if (active && active !== document.body && !root.current?.contains(active)) return;
    const rows = root.current?.querySelectorAll<HTMLElement>('[data-effect-row]') ?? [];
    const row = rows[Math.min(at, rows.length - 1)];
    (row?.querySelector<HTMLElement>('button') ?? add.current)?.focus();
  }, [ids]);

  const remove = (fx: EffectInstance, index: number) => {
    removedAt.current = index;
    // Refused, or nothing to send (the project no longer had it): no stack change follows, so no focus move is due.
    void removeEffect(stack, fx).then((removed) => {
      if (!removed) removedAt.current = null;
    });
  };

  /** Each effect's name; two of a kind are numbered ("Vignette (1)", "Vignette (2)") so each control says which. */
  const kinds = new Map<string, number>();
  for (const fx of effects) kinds.set(fx.effect, (kinds.get(fx.effect) ?? 0) + 1);
  const seen = new Map<string, number>();
  const names = effects.map((fx) => {
    const name = effectName(fx.effect);
    if ((kinds.get(fx.effect) ?? 0) < 2) return name;
    const n = (seen.get(fx.effect) ?? 0) + 1;
    seen.set(fx.effect, n);
    return t({
      id: 'effect.numbered',
      message: `${name} (${n})`,
      comment: 'An effect used more than once on a clip or layer, numbered in stack order: e.g. Vignette (2)',
    });
  });

  const fullNote = t({ id: 'effect.add.full', message: 'A clip or layer can have up to eight effects' });
  return (
    <div ref={root} className="fx-stack">
      {effects.length > 0 ? (
        <ul className="fx-list">
          {effects.map((fx, index) => (
            <EffectRow
              key={fx.id}
              stack={stack}
              fx={fx}
              index={index}
              name={names[index]}
              onRemove={() => remove(fx, index)}
            />
          ))}
        </ul>
      ) : (
        <p className="field-note">{t({ id: 'effect.none', message: 'No effects yet' })}</p>
      )}
      {/* One button in both states, so focus on it survives the eighth effect going in. */}
      <Menu open={menuOpen && !full} onOpenChange={(open) => setMenuOpen(open && !full)}>
        <MenuTrigger asChild>
          <AddButton
            ref={add}
            tooltip={full ? fullNote : t({ id: 'effect.add.tip', message: 'Add an effect to this clip or layer' })}
            // Inert at eight, focusable, and described by why: its tooltip, kept after a press closes the tip.
            describes={full}
            aria-disabled={full || undefined}
          >
            <Icon name="plus" />
            {t({ id: 'effect.add', message: 'Add effect', comment: 'Button and command' })}
          </AddButton>
        </MenuTrigger>
        <MenuContent>
          {EFFECTS.map((def) => (
            <MenuItem key={def.id} onSelect={() => void addEffect(stack, def.id)}>
              {contribLabel(def.id, 'effect')}
            </MenuItem>
          ))}
        </MenuContent>
      </Menu>
    </div>
  );
}

/** "Add effect": a small secondary button with its tooltip, taking the menu trigger's props. */
function AddButton({
  tooltip,
  describes,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { tooltip: string; describes: boolean; ref?: Ref<HTMLButtonElement> }) {
  return (
    <Tooltip label={tooltip} describes={describes}>
      <button type="button" tabIndex={0} {...rest} className="btn btn-secondary btn-sm fx-add">
        {children}
      </button>
    </Tooltip>
  );
}
