import { useLingui } from '@lingui/react/macro';
import { useRef } from 'react';
import type { ContribDef } from '../../bindings/ContribDef';
import type { Op } from '../../bindings/Op';
import type { ParamSpec } from '../../bindings/ParamSpec';
import type { ParamValue } from '../../bindings/ParamValue';
import { IconButton } from '../../design/IconButton';
import { Icon } from '../../layout/Icon';
import { deepEqual } from '../../lib/equal';
import type { Label } from '../../lib/labels';
import { commitLatest } from '../../state/latest';
import { Field } from './fields';
import { ParamColor } from './ParamColor';
import { ParamControl, SEGMENTS } from './ParamControl';
import { isShown } from './paramConditions';
import { defaultOf, isStored, valueOf, type ParamValues } from './paramFormat';
import { settingLabel } from './paramLabels';

interface ParamFieldsProps {
  def: ContribDef;
  values: ParamValues;
  /** The ops that set setting `key` to `value` (one undo step, labelled `commitLabel`). */
  opsFor: (key: string, value: ParamValue) => Op[];
  commitLabel: Label;
  /** The ops that take setting `key` back to its default; without them there is no Reset. */
  resetOps?: (key: string) => Op[];
  /** The stored values as the project has them when a change goes in (a point keeps its other axis from them). */
  valuesNow?: () => ParamValues;
}

/** Focus the setting's control in `el`: a segmented control's or swatches' checked option, else its first control. */
function focusControl(el: HTMLElement | null) {
  const target =
    el?.querySelector<HTMLElement>('[role="radio"][tabindex="0"], [role="radio"][aria-checked="true"]') ??
    el?.querySelector<HTMLElement>('[role="slider"], button:not([disabled]), input');
  target?.focus();
}

/**
 * Every setting of a contribution (an effect, a generator), generated from its schema in schema order: one labelled
 * row each, and a Reset beside a stored value that differs from the default. A setting whose `visibleWhen` doesn't
 * hold is hidden (`isShown`); its stored value stays.
 */
export function ParamFields({ def, values, ...rest }: ParamFieldsProps) {
  return (
    <>
      {def.params
        .filter((spec) => isShown(def, spec, values))
        .map((spec) => (
          <ParamField key={spec.key} spec={spec} values={values} {...rest} />
        ))}
    </>
  );
}

function ParamField({
  spec,
  values,
  opsFor,
  commitLabel,
  resetOps,
  valuesNow,
}: Omit<ParamFieldsProps, 'def'> & { spec: ParamSpec }) {
  const { t } = useLingui();
  const controlRef = useRef<HTMLDivElement>(null);
  const label = settingLabel(spec);
  const changed = resetOps && isStored(spec, values) && !deepEqual(values[spec.key], defaultOf(spec));
  const reset = changed && (
    <IconButton
      className="icon-btn-sm"
      icon={<Icon name="undo" />}
      label={t({ id: 'inspector.param.reset', message: `Reset ${label}`, comment: '{label}: a setting, e.g. Radius' })}
      onClick={() => {
        // Reset goes once the value is the default: focus goes to the setting, not to the page.
        focusControl(controlRef.current);
        void commitLatest(commitLabel, () => resetOps(spec.key));
      }}
    />
  );
  const now = valuesNow && (() => valueOf(spec, valuesNow()));
  if (spec.type === 'color') {
    const value = valueOf(spec, values);
    return (
      <div ref={controlRef} className="param-rows">
        <ParamColor
          label={label}
          value={typeof value === 'string' ? value : spec.default}
          alpha={spec.alpha}
          opsFor={(color) => opsFor(spec.key, color)}
          commitLabel={commitLabel}
          valueNow={now && (() => {
            const color = now();
            return typeof color === 'string' ? color : spec.default;
          })}
          reset={reset}
        />
      </div>
    );
  }
  const control = (
    <div ref={controlRef} className="param-control">
      <ParamControl
        spec={spec}
        value={valueOf(spec, values)}
        label={label}
        opsFor={(value) => opsFor(spec.key, value)}
        commitLabel={commitLabel}
        valueNow={now}
      />
      {reset}
    </div>
  );
  // A segmented choice and a point take the row's width under their name; the rest sit beside it.
  const stack = spec.type === 'point' || (spec.type === 'choice' && spec.options.length <= SEGMENTS);
  return (
    <Field label={label} stack={stack}>
      {control}
    </Field>
  );
}
