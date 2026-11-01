import { useLingui } from '@lingui/react/macro';
import { useIntlLocale } from '../../i18n/react';
import clipSettings from '../../lib/clip-settings.json';
import { formatNumber } from '../../lib/format';
import { Field, Select } from './fields';

/**
 * The speeds a clip or a video layer offers; its length follows (the model rescales it onto the same footage). The
 * model's list (`klipolo_model::clip_settings::SPEEDS`), which chat's setClip takes too.
 */
const SPEEDS: number[] = clipSettings.speeds;

/**
 * The speeds to choose from for a clip or layer playing at `current`: `SPEEDS`, and `current` when it is another, so the
 * select always shows what is set.
 */
export function speedChoices(current: number): number[] {
  return SPEEDS.includes(current) ? SPEEDS : [...SPEEDS, current].sort((a, b) => a - b);
}

/** Speed: a select of 0.5x to 2x ("1.25×"), and `speed` when it is another; each pick is one change (`onPick`). */
export function SpeedField({ speed, onPick }: { speed: number; onPick: (speed: number) => void }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const label = t({ id: 'speed.label', message: 'Speed' });
  return (
    <Field label={label}>
      <Select
        label={label}
        value={String(speed)}
        choices={speedChoices(speed).map((value) => ({
          value: String(value),
          label: `${formatNumber(locale, value)}×`,
        }))}
        onPick={(value) => onPick(Number(value))}
      />
    </Field>
  );
}
