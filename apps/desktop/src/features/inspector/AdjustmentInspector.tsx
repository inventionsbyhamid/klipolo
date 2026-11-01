import { useLingui } from '@lingui/react/macro';
import type { Item } from '../../bindings/Item';
import { TimingField } from './common';
import { EffectsButton } from './EffectsButton';

/** An adjustment layer's fields: it is its effects, applied to everything under it while it lasts; then its Timing. */
export function AdjustmentInspector({ item }: { item: Item }) {
  const { t } = useLingui();
  return (
    <>
      <p className="field-note">
        {t({
          id: 'adjustment.hint',
          message: "Effects here change what's under this layer",
          comment: 'The card of an adjustment layer',
        })}
      </p>
      <EffectsButton target={{ type: 'item', id: item.id }} effects={item.effects} />
      <TimingField item={item} />
    </>
  );
}
