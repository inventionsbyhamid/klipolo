import { useLingui } from '@lingui/react/macro';
import type { Anchor } from '../../bindings/Anchor';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import { FORMAT_SIZE } from '../../lib/layout';
import { formatPrecise } from '../../lib/time';
import { Field, PositionGrid, Select, ValueField } from './fields';
import {
  animationOf,
  parseTime,
  positionOf,
  positionedLayout,
  withAnimation,
  type InChoice,
  type OutChoice,
} from './inspectorOps';
import { commitLatest, editLayout } from './latest';

/** The shortest a layer can be made in the inspector (the model's least is 10 ms). */
const MIN_LENGTH_US = 100_000;

/** Position: the 3 x 3 grid, in the format shown (an override in a format other than the primary). */
export function PositionField({ item, format, primary }: { item: Item; format: FormatKind; primary: FormatKind }) {
  const { t } = useLingui();
  const names: Record<Anchor, string> = {
    topLeft: t({ id: 'position.topLeft', message: 'Top left' }),
    topCenter: t({ id: 'position.topCenter', message: 'Top' }),
    topRight: t({ id: 'position.topRight', message: 'Top right' }),
    middleLeft: t({ id: 'position.middleLeft', message: 'Left' }),
    center: t({ id: 'position.center', message: 'Middle' }),
    middleRight: t({ id: 'position.middleRight', message: 'Right' }),
    bottomLeft: t({ id: 'position.bottomLeft', message: 'Bottom left' }),
    bottomCenter: t({ id: 'position.bottomCenter', message: 'Bottom' }),
    bottomRight: t({ id: 'position.bottomRight', message: 'Bottom right' }),
  };
  const label = t({ id: 'inspector.position', message: 'Position' });
  return (
    <Field label={label}>
      <PositionGrid
        label={label}
        names={names}
        value={positionOf(item.layout, format, primary)}
        onPick={(anchor) =>
          void editLayout('layout.position.undo', item, (layout) =>
            positionedLayout(layout, anchor, format, primary, FORMAT_SIZE[format]),
          )
        }
      />
    </Field>
  );
}

/** Animation: how it comes in and goes out (the In and Out selects), one undo step each. */
export function AnimationField({ item, typewriter }: { item: Item; typewriter: boolean }) {
  const { t } = useLingui();
  const none = t({ id: 'animation.none', message: 'None', comment: 'No animation' });
  const fade = t({ id: 'animation.fade', message: 'Fade' });
  const pop = t({ id: 'animation.pop', message: 'Pop' });
  const ins: { value: InChoice; label: string }[] = [
    { value: 'none', label: none },
    { value: 'fade', label: fade },
    { value: 'pop', label: pop },
    { value: 'slideUp', label: t({ id: 'animation.slideUp', message: 'Slide up' }) },
    ...(typewriter ? [{ value: 'typewriter' as const, label: t({ id: 'animation.typewriter', message: 'Typewriter' }) }] : []),
  ];
  const outs: { value: OutChoice; label: string }[] = [
    { value: 'none', label: none },
    { value: 'fade', label: fade },
    { value: 'pop', label: pop },
    { value: 'slideDown', label: t({ id: 'animation.slideDown', message: 'Slide down' }) },
  ];
  const other = t({ id: 'animation.other', message: 'Other', comment: 'An animation set from chat' });
  const set = (end: 'enter' | 'exit', choice: InChoice | OutChoice) =>
    void editLayout('text.animation.undo', item, (layout) => ({
      motion: withAnimation(layout, end, choice, item.durationUs),
    }));
  const enter = animationOf(item.layout.motion, 'enter');
  const exit = animationOf(item.layout.motion, 'exit');
  return (
    <Field label={t({ id: 'inspector.animation', message: 'Animation' })} stack>
      <div className="two">
        <Select
          label={t({ id: 'animation.in', message: 'In', comment: 'How it comes in' })}
          prefix={t({ id: 'animation.in', message: 'In', comment: 'How it comes in' })}
          value={(enter ?? 'other') as InChoice}
          choices={enter === null ? [...ins, { value: 'other' as InChoice, label: other }] : ins}
          onPick={(v) => set('enter', v)}
        />
        <Select
          label={t({ id: 'animation.out', message: 'Out', comment: 'How it goes out' })}
          prefix={t({ id: 'animation.out', message: 'Out', comment: 'How it goes out' })}
          value={(exit ?? 'other') as OutChoice}
          choices={exit === null ? [...outs, { value: 'other' as OutChoice, label: other }] : outs}
          onPick={(v) => set('exit', v)}
        />
      </div>
    </Field>
  );
}

/** Timing: when it starts and how long it lasts, each typed (Enter or leaving the field commits it). */
export function TimingField({ item }: { item: Item }) {
  const { t } = useLingui();
  return (
    <Field label={t({ id: 'inspector.timing', message: 'Timing' })} stack>
      <div className="two">
        <ValueField
          label={t({ id: 'timing.starts', message: 'Starts' })}
          value={formatPrecise(item.startUs)}
          parse={parseTime}
          onCommit={(startUs) =>
            void commitLatest('timing.start.undo', () => [
              { type: 'moveItem', id: item.id, trackId: null, startUs: Math.max(0, startUs) },
            ])
          }
        />
        <ValueField
          label={t({ id: 'timing.lasts', message: 'Lasts' })}
          value={`${(item.durationUs / 1_000_000).toFixed(1)} s`}
          parse={parseTime}
          onCommit={(durationUs) =>
            void commitLatest('timing.length.undo', () => [
              { type: 'updateItem', id: item.id, patch: { durationUs: Math.max(MIN_LENGTH_US, durationUs) } },
            ])
          }
        />
      </div>
    </Field>
  );
}
