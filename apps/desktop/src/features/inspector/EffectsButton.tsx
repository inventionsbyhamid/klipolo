import { useLingui } from '@lingui/react/macro';
import type { EffectInstance } from '../../bindings/EffectInstance';
import type { EffectTarget } from '../../bindings/EffectTarget';
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from '../../design/Popover';
import { Tooltip } from '../../design/Tooltip';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatNumber } from '../../lib/format';
import { EffectStack } from './EffectStack';

interface EffectsButtonProps {
  target: EffectTarget;
  effects: EffectInstance[];
}

/**
 * A card's Effects button ("Effects 2" when there are some): opens the clip's or layer's effect stack in a popover
 * at the card's width, its right edge on the card's, so it covers the card rather than the video. Its sliders and
 * colours commit what they hold when it closes (their bursts flush as they unmount).
 */
export function EffectsButton({ target, effects }: EffectsButtonProps) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const title = t({ id: 'effect.stack', message: 'Effects', comment: 'The effects on a clip or layer' });
  const count = effects.length;
  const shown = formatNumber(locale, count);
  return (
    <Popover>
      {/* The card's row is what the popover lines up with: its right edge on the card's, over the card. */}
      <PopoverAnchor asChild>
        <div className="insp-effects-row">
          <Tooltip label={t({ id: 'effect.stack.tip', message: 'Add and change effects' })}>
            <PopoverTrigger asChild>
              <button
                type="button"
                tabIndex={0}
                className="btn btn-secondary btn-sm"
                aria-label={
                  count > 0
                    ? t({ id: 'effect.stack.count', message: `Effects ${shown}`, comment: 'Button: {shown} effects are on it' })
                    : undefined
                }
              >
                <Icon name="sparkle" />
                {t({ id: 'effect.stack.button', message: 'Effects', comment: 'Short button: opens the effects on a clip or layer' })}
                {count > 0 && (
                  <span aria-hidden className="fx-count">
                    {shown}
                  </span>
                )}
              </button>
            </PopoverTrigger>
          </Tooltip>
        </div>
      </PopoverAnchor>
      <PopoverContent
        aria-label={title}
        side="bottom"
        align="end"
        className="fx-pop"
        // Focus the panel, not its first control, so opening it doesn't pop a tooltip; Tab goes on to the controls.
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          if (e.currentTarget instanceof HTMLElement) e.currentTarget.focus();
        }}
      >
        <EffectStack target={target} effects={effects} />
      </PopoverContent>
    </Popover>
  );
}
