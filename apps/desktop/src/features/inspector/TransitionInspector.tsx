import { useLingui } from '@lingui/react/macro';
import type { Clip } from '../../bindings/Clip';
import type { Op } from '../../bindings/Op';
import type { ParamSpec } from '../../bindings/ParamSpec';
import type { ParamValue } from '../../bindings/ParamValue';
import type { Project } from '../../bindings/Project';
import type { Transition } from '../../bindings/Transition';
import { IconButton } from '../../design/IconButton';
import { Segmented } from '../../design/Segmented';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { builtinsOf, contribLabel, isInstalled } from '../../lib/contrib';
import { formatSeconds } from '../../lib/format';
import { labelText } from '../../lib/labels';
import { maxTransitionUs, MIN_TRANSITION_US, transitionLengthUs } from '../../lib/transitions';
import { commitLatest } from '../../state/latest';
import { useProject } from '../../state/project';
import { transitionClipId, useSelection } from '../../state/selection';
import { removeTransition, transitionName, transitionNow, transitionOp } from '../timeline/commands';
import { cardEscape } from './common';
import { Field, Select, Swatches } from './fields';
import { textSwatches } from './TextInspector';
import { ToolSlider } from './ToolSlider';

/** The selected transition the card shows: the clip before its cut, that clip's place and its `transitionOut`. */
export interface InspectedTransition {
  project: Project;
  clip: Clip;
  index: number;
  transition: Transition;
}

/** The transition selected (`transition:<clip id>`), while its clip still has one into a next clip; else null. */
export function inspectedTransition(
  project: Project | undefined,
  selectedId: string | null,
): InspectedTransition | null {
  const clipId = transitionClipId(selectedId);
  if (!project || clipId === null) return null;
  const main = project.timeline.main;
  const index = main.findIndex((clip) => clip.id === clipId);
  const transition = index >= 0 && index + 1 < main.length ? main[index].transitionOut : undefined;
  return transition ? { project, clip: main[index], index, transition } : null;
}

/** The selected transition the card shows, or null. */
export function useInspectedTransition(): InspectedTransition | null {
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  return inspectedTransition(project, selectedId);
}

const percent = (v: number) => `${Math.round(v * 100)}%`;
/** A choice's stored key ("top-left") as it reads ("Top left"), for a key `useChoiceLabel` doesn't word. */
const keyLabel = (option: string) => {
  const words = option.replace(/[-_]/g, ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
};
/** A segmented control holds up to this many choices; more go in a select. */
const SEGMENTS = 4;

/**
 * The transition card, in the left column: a selected transition's type, its length (on the frame grid, up
 * to what the footage either side allows), its settings from the type's schema, and Remove. Each change is one
 * `setTransition`, one undo step, built from the transition as the project has it when it goes in (`transitionNow`),
 * so a quick second change (a length while a new type lands) keeps the first. The length and the sliders dry-run in
 * the preview while they move.
 */
export function TransitionInspector({ project, clip, index, transition }: InspectedTransition) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const fps = project.settings.fps;
  const maxUs = maxTransitionUs(project, index);
  const def = builtinsOf('transition').find((d) => d.id === transition.effect);
  const now = () => transitionNow(clip.id)?.transition ?? transition;
  const set = (next: Partial<Transition>): Op[] => [transitionOp(clip.id, { ...now(), ...next })];
  /** A length in seconds as the model takes it: whole frames within what the footage allows now. */
  const lengthUs = (seconds: number) => {
    const at = transitionNow(clip.id);
    const max = at ? maxTransitionUs(at.project, at.index) : maxUs;
    return transitionLengthUs(seconds * 1e6, max, fps) || now().durationUs;
  };
  const choices = builtinsOf('transition').map((d) => ({ value: d.id, label: contribLabel(d.id, 'transition') }));
  // One this build lacks stays listed ("Missing transition"), so the select shows what is set.
  if (!isInstalled(transition.effect)) choices.push({ value: transition.effect, label: transitionName(transition) });
  const shortest = transitionLengthUs(MIN_TRANSITION_US, maxUs, fps) || MIN_TRANSITION_US;
  const type = t({ id: 'transition.type', message: 'Type', comment: 'Which transition: Dissolve, Push, Wipe' });
  const length = t({ id: 'transition.length', message: 'Length', comment: 'How long a transition lasts' });
  const close = () => useSelection.getState().select(null);
  return (
    <aside
      className="inspector"
      aria-label={t({ id: 'inspector.transition.label', message: 'Transition settings' })}
      onKeyDown={cardEscape(close)}
    >
      <div className="insp-head">
        <h3>
          <Icon name="blend" />
          {t({ id: 'inspector.transition', message: 'Transition' })}
        </h3>
        <IconButton
          icon={<Icon name="x" />}
          label={t({ id: 'inspector.close', message: 'Close' })}
          shortcut="Esc"
          onClick={close}
        />
      </div>
      <div className="insp-body">
        <Field label={type}>
          <Select
            label={type}
            value={transition.effect}
            choices={choices}
            // A new type starts from its own defaults: the old type's settings mean nothing to it.
            onPick={(effect) => void commitLatest('transition.change.undo', () => set({ effect, params: {} }))}
          />
        </Field>
        <Field label={length}>
          <ToolSlider
            label={length}
            commitLabel="transition.length.undo"
            value={transition.durationUs / 1e6}
            min={Math.min(shortest, transition.durationUs) / 1e6}
            max={Math.max(maxUs, transition.durationUs) / 1e6}
            step={1 / fps}
            format={(v) => formatSeconds(locale, v)}
            ops={(v) => set({ durationUs: lengthUs(v) })}
          />
        </Field>
        {def?.params.map((spec) => (
          <Setting
            key={`${def.id}:${spec.key}`}
            spec={spec}
            value={transition.params[spec.key]}
            ops={(value) => set({ params: { ...now().params, [spec.key]: value } })}
          />
        ))}
        <button
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          className="btn btn-secondary btn-sm insp-remove"
          onClick={() => removeTransition(clip.id, { focusCut: true })}
        >
          <Icon name="trash" />
          {labelText('transition.remove')}
        </button>
      </div>
    </aside>
  );
}

/** A setting's name in the language shown: the ones the transitions have, else its schema's label. */
function useSettingLabel(spec: ParamSpec): string {
  const { t } = useLingui();
  if (spec.key === 'color') return t({ id: 'transition.setting.color', message: 'Color' });
  if (spec.key === 'direction')
    return t({ id: 'transition.setting.direction', message: 'Direction', comment: 'Which way a push or wipe goes' });
  if (spec.key === 'softness')
    return t({ id: 'transition.setting.softness', message: 'Softness', comment: 'How soft a wipe’s edge is' });
  return spec.label;
}

/** A choice's name in the language shown: the directions the transitions have, else its key as it reads. */
function useChoiceLabel(): (option: string) => string {
  const { t } = useLingui();
  const words: Record<string, string> = {
    left: t({ id: 'transition.direction.left', message: 'Left', comment: 'A push or wipe moving to the left' }),
    right: t({ id: 'transition.direction.right', message: 'Right', comment: 'A push or wipe moving to the right' }),
    up: t({ id: 'transition.direction.up', message: 'Up', comment: 'A push or wipe moving up' }),
    down: t({ id: 'transition.direction.down', message: 'Down', comment: 'A push or wipe moving down' }),
  };
  return (option) => words[option] ?? keyLabel(option);
}

/**
 * One of a transition's settings, by its schema's type: a slider for a number (dry-run while it moves, one change per
 * drag), swatches for a colour, a segmented control (or a select, past four choices) for a choice, On and Off for a
 * switch. Each pick is one change. A type with no control here (none of the built-ins has one) shows nothing.
 */
function Setting({
  spec,
  value,
  ops,
}: {
  spec: ParamSpec;
  value: ParamValue | undefined;
  ops: (value: ParamValue) => Op[];
}) {
  const { t } = useLingui();
  const label = useSettingLabel(spec);
  const choiceLabel = useChoiceLabel();
  const pick = (next: ParamValue) => void commitLatest('transition.change.undo', () => ops(next));
  switch (spec.type) {
    case 'float':
    case 'int': {
      const now = typeof value === 'number' && Number.isFinite(value) ? value : spec.default;
      const step =
        spec.type === 'int' ? 1 : (spec.step ?? (spec.unit === 'percent' ? 0.01 : (spec.max - spec.min) / 100));
      const shown = (v: number) =>
        spec.type === 'float' && spec.unit === 'percent' ? percent(v) : String(Math.round(v * 100) / 100);
      return (
        <Field label={label}>
          <ToolSlider
            label={label}
            commitLabel="transition.change.undo"
            value={now}
            min={spec.min}
            max={spec.max}
            step={step}
            format={shown}
            ops={ops}
          />
        </Field>
      );
    }
    case 'choice': {
      const now = typeof value === 'string' && spec.options.includes(value) ? value : spec.default;
      const options = spec.options.map((option) => ({ value: option, label: choiceLabel(option) }));
      return options.length <= SEGMENTS ? (
        <Field label={label} stack>
          <Segmented label={label} value={now} options={options} fill onChange={pick} />
        </Field>
      ) : (
        <Field label={label}>
          <Select label={label} value={now} choices={options} onPick={pick} />
        </Field>
      );
    }
    case 'color': {
      const now = typeof value === 'string' ? value : spec.default;
      return (
        <Field label={label}>
          <Swatches label={label} value={now} swatches={textSwatches(t)} onPick={pick} />
        </Field>
      );
    }
    case 'bool': {
      const now = typeof value === 'boolean' ? value : spec.default;
      return (
        <Field label={label}>
          <Segmented
            label={label}
            value={now ? 'on' : 'off'}
            options={[
              { value: 'on', label: t({ id: 'transition.setting.on', message: 'On' }) },
              { value: 'off', label: t({ id: 'transition.setting.off', message: 'Off' }) },
            ]}
            fill
            onChange={(v) => pick(v === 'on')}
          />
        </Field>
      );
    }
    default:
      return null;
  }
}
