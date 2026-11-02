import { useLingui } from '@lingui/react/macro';
import { ChevronLeft } from 'lucide-react';
import type { KeyboardEvent } from 'react';
import type { Anchor } from '../../bindings/Anchor';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import { IconButton } from '../../design/IconButton';
import { FORMAT_SIZE } from '../../lib/layout';
import { useLibraryPanel } from '../../state/libraryPanel';
import { formatPrecise } from '../../lib/time';
import { Field, PositionGrid, Select, ValueField } from './fields';
import {
  animationOf,
  emphasisOf,
  parseTime,
  positionOf,
  positionedLayout,
  withAnimation,
  withEmphasis,
  type EmphasisChoice,
  type InChoice,
  type OutChoice,
} from './inspectorOps';
import { commitLatest, editLayout } from '../../state/latest';
import { keySlackUs } from '../../state/keyEdits';
import { usePlayback } from '../../state/playback';
import { placeWithKeys } from '../../lib/keyedPlace';

/**
 * A card's Escape (its `aside`'s key handler): `close` the card, for an Escape pressed in the card itself. Not one a
 * field keeps (a text or time field puts its value back), not one already handled, and not one from a menu the card
 * opened: menus open in a portal, outside the card's own elements, and React still passes their keys up to it, so
 * Escape there closes the menu alone.
 */
export function cardEscape(close: () => void) {
  return (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== 'Escape' || e.defaultPrevented || !(e.target instanceof Node)) return;
    if (!e.currentTarget.contains(e.target)) return;
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
    e.stopPropagation();
    close();
  };
}

/**
 * Back to library, at the start of a card's head: the left column shows the library again (its view, tab, search and
 * scroll as they were) and the selection stays. Picking anything, even the same thing again, brings the card back.
 */
export function BackToLibrary() {
  const { t } = useLingui();
  return (
    <IconButton
      icon={ChevronLeft}
      className="insp-back"
      label={t({ id: 'inspector.backToLibrary', message: 'Back to library' })}
      onClick={() => useLibraryPanel.getState().stepAside()}
    />
  );
}

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
            // With position keys, the layer as drawn at the playhead goes there, and the key there takes the move.
            placeWithKeys(
              layout,
              (drawn) => positionedLayout(drawn, anchor, format, primary, FORMAT_SIZE[format]),
              Math.min(item.durationUs, Math.max(0, usePlayback.getState().playheadUs - item.startUs)),
              item.durationUs,
              primary,
              format,
              keySlackUs(),
            ),
          )
        }
      />
    </Field>
  );
}

/**
 * Animation: how it comes in and goes out (the In and Out selects) and how it moves while it shows (Loop), as chat's
 * `animate` sets them, one undo step each. A preset the selects don't offer reads "Other".
 */
export function AnimationField({ item, typewriter }: { item: Item; typewriter: boolean }) {
  const { t } = useLingui();
  const none = t({ id: 'animation.none', message: 'None', comment: 'No animation' });
  const fade = t({ id: 'animation.fade', message: 'Fade' });
  const pop = t({ id: 'animation.pop', message: 'Pop' });
  const zoom = t({ id: 'animation.zoom', message: 'Zoom' });
  const slides = [
    { value: 'slideUp' as const, label: t({ id: 'animation.slideUp', message: 'Slide up' }) },
    { value: 'slideDown' as const, label: t({ id: 'animation.slideDown', message: 'Slide down' }) },
    { value: 'slideLeft' as const, label: t({ id: 'animation.slideLeft', message: 'Slide left' }) },
    { value: 'slideRight' as const, label: t({ id: 'animation.slideRight', message: 'Slide right' }) },
  ];
  const ins: { value: InChoice; label: string }[] = [
    { value: 'none', label: none },
    { value: 'fade', label: fade },
    { value: 'pop', label: pop },
    { value: 'zoom', label: zoom },
    { value: 'bounce', label: t({ id: 'animation.bounce', message: 'Bounce' }) },
    { value: 'blur', label: t({ id: 'animation.blur', message: 'Blur', comment: 'Comes in sharpening from a blur' }) },
    ...slides,
    ...(typewriter ? [{ value: 'typewriter' as const, label: t({ id: 'animation.typewriter', message: 'Typewriter' }) }] : []),
  ];
  const outs: { value: OutChoice; label: string }[] = [
    { value: 'none', label: none },
    { value: 'fade', label: fade },
    { value: 'pop', label: pop },
    { value: 'zoom', label: zoom },
    ...slides,
  ];
  const loops: { value: EmphasisChoice; label: string }[] = [
    { value: 'none', label: none },
    { value: 'pulse', label: t({ id: 'animation.pulse', message: 'Pulse' }) },
    { value: 'shake', label: t({ id: 'animation.shake', message: 'Shake' }) },
    { value: 'wiggle', label: t({ id: 'animation.wiggle', message: 'Wiggle' }) },
  ];
  const other = t({ id: 'animation.other', message: 'Other', comment: 'An animation set from chat' });
  const set = (end: 'enter' | 'exit', choice: InChoice | OutChoice) =>
    void editLayout('text.animation.undo', item, (layout) => ({
      motion: withAnimation(layout, end, choice, item.durationUs),
    }));
  const setLoop = (choice: EmphasisChoice) =>
    void editLayout('text.animation.undo', item, (layout) => ({
      motion: withEmphasis(layout, choice, item.durationUs),
    }));
  const enter = animationOf(item.layout.motion, 'enter');
  const exit = animationOf(item.layout.motion, 'exit');
  const loop = emphasisOf(item.layout.motion);
  const loopLabel = t({ id: 'animation.loop', message: 'Loop', comment: 'How it moves while it shows' });
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
      <Select
        label={loopLabel}
        prefix={loopLabel}
        value={(loop ?? 'other') as EmphasisChoice}
        choices={loop === null ? [...loops, { value: 'other' as EmphasisChoice, label: other }] : loops}
        onPick={(v) => setLoop(v)}
      />
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
