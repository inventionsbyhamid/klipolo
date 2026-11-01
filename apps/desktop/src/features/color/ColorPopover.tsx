import { useLingui } from '@lingui/react/macro';
import type { JSX } from 'react';
import { HexInput } from '../../design/color/HexInput';
import { PopoverContent } from '../../design/Popover';

interface ColorPopoverProps {
  label: string;
  value: string | null;
  alpha: boolean;
  /** A custom colour picked: the hex field's. */
  onCustom(hex: string): void;
}

/**
 * The color popover: anchored to the "+" that opened it and non-modal, so the video stays in view and shows each
 * colour live. A hex field takes any colour.
 */
export function ColorPopover({ label, value, alpha, onCustom }: ColorPopoverProps): JSX.Element {
  const { t } = useLingui();
  return (
    <PopoverContent aria-label={label} side="bottom" align="start" className="color-popover">
      <HexInput
        label={t({ id: 'color.hex', message: 'Hex color' })}
        value={value}
        alpha={alpha}
        invalidText={
          alpha
            ? t({
                id: 'color.hex.invalid.alpha',
                message: 'Type 3 or 6 hex digits, or 8 with opacity, like FFD02E80.',
                comment: 'A color setting with opacity: the last two of 8 digits are the opacity',
              })
            : t({ id: 'color.hex.invalid', message: 'Type 3 or 6 hex digits, like FFD02E.' })
        }
        noAlphaText={t({
          id: 'color.hex.noAlpha',
          message: 'This setting has no opacity, so the color is used without it.',
          comment: 'Eight hex digits (a color with opacity) went into a color setting that has none',
        })}
        onCommit={onCustom}
      />
    </PopoverContent>
  );
}
