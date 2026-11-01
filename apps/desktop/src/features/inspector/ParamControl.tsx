import { useLingui } from '@lingui/react/macro';
import { useState } from 'react';
import type { AssetKind } from '../../bindings/AssetKind';
import type { FontRef } from '../../bindings/FontRef';
import type { Op } from '../../bindings/Op';
import type { ParamSpec } from '../../bindings/ParamSpec';
import type { ParamValue } from '../../bindings/ParamValue';
import type { PointN } from '../../bindings/PointN';
import { Segmented } from '../../design/Segmented';
import { Toggle } from '../../design/Toggle';
import { useBundledFamilies } from '../../lib/fonts';
import type { Label } from '../../lib/labels';
import { commitToggle } from '../../state/commit';
import { commitLatest } from '../../state/latest';
import { useProject } from '../../state/project';
import { Field, Select } from './fields';
import { choiceLabel, defaultOf, fits, formatParam, sliderRange } from './paramFormat';
import { FAMILIES } from './TextInspector';
import { ToolSlider } from './ToolSlider';

export interface ParamControlProps {
  spec: Exclude<ParamSpec, { type: 'color' }>;
  /** The value shown: stored, else the schema's default (`valueOf`). */
  value: ParamValue | null;
  label: string;
  /** The ops that set the setting to `value`. */
  opsFor: (value: ParamValue) => Op[];
  /** The undo label of every change this control makes. */
  commitLabel: Label;
  /**
   * The value as the project has it when a change goes in (a point's slider keeps the other axis from it, so a quick
   * change to one axis after the other doesn't put the first back); `value` when not given.
   */
  valueNow?: () => ParamValue | null;
  /** What a switch flips, so its quick presses are counted apart from other switches' (`commitToggle`). */
  toggleId?: string;
}

const MIDDLE: PointN = { x: 0.5, y: 0.5 };

/** Segmented for up to this many options; a select for more. */
export const SEGMENTS = 4;

const percent = (v: number) => `${Math.round(v * 100)}%`;

/**
 * The control for one setting, chosen by its schema's type alone (no per-effect code): a slider for numbers, a toggle,
 * two sliders for a point, a segmented control or a select for a choice, a text field, or a select of files or fonts
 * (a colour is `ParamColor`). Sliders are dry-run while they move and commit one undo step per gesture (`ToolSlider`);
 * a pick commits at once. Ops are built when the change goes in (`commitLatest`, a burst's builder), so they build on
 * a change another control has just made.
 */
export function ParamControl({ spec, value, label, opsFor, commitLabel, valueNow, toggleId }: ParamControlProps) {
  const { t } = useLingui();
  const now = fits(spec, value ?? undefined) ? value : defaultOf(spec);
  const pick = (next: ParamValue) => void commitLatest(commitLabel, () => opsFor(next));
  switch (spec.type) {
    case 'float':
    case 'int':
    case 'angle': {
      const range = sliderRange(spec)!;
      return (
        <ToolSlider
          label={label}
          commitLabel={commitLabel}
          value={typeof now === 'number' ? now : range.min}
          {...range}
          format={(v) => formatParam(spec, v)}
          ops={(v) => opsFor(v)}
        />
      );
    }
    case 'bool':
      return (
        <Toggle
          aria-label={label}
          checked={now === true}
          // Each press flips what the project has as it goes in, so two quick presses turn it on and back off.
          onCheckedChange={() =>
            void commitToggle(
              () => (valueNow ? valueNow() : now) === true,
              () => commitLabel,
              (on) => opsFor(on),
              { id: toggleId },
            )
          }
        />
      );
    case 'point': {
      const point = (now as PointN | null) ?? MIDDLE;
      /** The point as the project has it now: each slider is its own change, keeping the other axis. */
      const latest = (): PointN => {
        const stored = valueNow?.();
        return fits(spec, stored ?? undefined) ? (stored as PointN) : point;
      };
      const slider = (axis: 'x' | 'y', short: string, name: string) => (
        <Field label={short}>
          <ToolSlider
            label={name}
            commitLabel={commitLabel}
            value={point[axis]}
            // A point chat set past the frame (the model takes -1 to 2) shows where it is.
            min={Math.min(0, point[axis])}
            max={Math.max(1, point[axis])}
            step={0.01}
            format={percent}
            ops={(v) => opsFor({ ...latest(), [axis]: v })}
          />
        </Field>
      );
      return (
        <div role="group" aria-label={label} className="param-point">
          {slider(
            'x',
            t({ id: 'param.point.across.short', message: 'Across', comment: 'Where a point is, left to right' }),
            t({
              id: 'param.point.across',
              message: `${label} across`,
              comment: 'Slider: where a point is from left to right; {label} is its name, e.g. Center',
            }),
          )}
          {slider(
            'y',
            t({ id: 'param.point.down.short', message: 'Down', comment: 'Where a point is, top to bottom' }),
            t({
              id: 'param.point.down',
              message: `${label} down`,
              comment: 'Slider: where a point is from top to bottom; {label} is its name, e.g. Center',
            }),
          )}
        </div>
      );
    }
    case 'choice': {
      const options = spec.options.map((option) => ({ value: option, label: choiceLabel(option) }));
      const current = typeof now === 'string' ? now : spec.default;
      return options.length <= SEGMENTS ? (
        <Segmented label={label} value={current} options={options} fill onChange={pick} />
      ) : (
        <Select label={label} value={current} choices={options} onPick={pick} />
      );
    }
    case 'text':
      return <WordsSetting label={label} value={typeof now === 'string' ? now : ''} maxLen={spec.maxLen} onPick={pick} />;
    case 'asset':
      return <AssetSetting label={label} kind={spec.kind} value={typeof now === 'string' ? now : null} onPick={pick} />;
    case 'font':
      return <FontSetting label={label} value={(now as FontRef | null) ?? spec.default} onPick={pick} />;
    // A table's rows (a motion graphic's data) are written in chat: the card has no control for them.
    case 'table':
      break;
    default: {
      // A kind the bindings gain fails `tsc` here; at run time (a pack newer than this build) it says so.
      const unknown: never = spec;
      void unknown;
    }
  }
  return (
    <span className="field-note">
      {t({
        id: 'inspector.param.unsupported',
        message: "Can't be changed in this version",
        comment: 'A setting of a kind this version has no control for',
      })}
    </span>
  );
}

/**
 * Words, committed on Enter or when focus leaves; Escape puts back what was there. At most `maxLen` characters. Typing
 * here never reaches the editor's shortcuts.
 */
function WordsSetting({
  label,
  value,
  maxLen,
  onPick,
}: {
  label: string;
  value: string;
  maxLen: number;
  onPick: (value: string) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const done = () => {
    if (draft === null) return;
    setDraft(null);
    if (draft !== value) onPick(draft);
  };
  return (
    <input
      type="text"
      className="input input-sm"
      aria-label={label}
      value={draft ?? value}
      // Words not yet kept: Escape puts them back before it closes anything around them.
      data-draft={draft !== null && draft !== value ? '' : undefined}
      maxLength={maxLen}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={done}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.nativeEvent.isComposing || e.keyCode === 229) return;
        if (e.key === 'Enter') {
          e.preventDefault();
          done();
        } else if (e.key === 'Escape') {
          e.preventDefault();
          setDraft(null);
        }
      }}
    />
  );
}

/** One of the project's files of `kind` (an image for a texture, a video), picked from a select. */
function AssetSetting({
  label,
  kind,
  value,
  onPick,
}: {
  label: string;
  kind: AssetKind;
  value: string | null;
  onPick: (value: string) => void;
}) {
  const { t } = useLingui();
  const assets = useProject((s) => s.snapshot?.project.assets);
  const choices = (assets ?? []).filter((a) => a.kind === kind).map((a) => ({ value: a.id, label: a.name }));
  const chosen = choices.some((c) => c.value === value);
  const none = t({ id: 'inspector.param.asset.none', message: 'Choose a file', comment: 'No file chosen yet' });
  return (
    <Select
      label={label}
      value={chosen ? value! : ''}
      choices={chosen ? choices : [{ value: '', label: none }, ...choices]}
      onPick={(id) => id && id !== value && onPick(id)}
    />
  );
}

/** A font setting: one of the bundled families, stored as a `FontRef`. */
function FontSetting({ label, value, onPick }: { label: string; value: FontRef; onPick: (font: FontRef) => void }) {
  const families = useBundledFamilies(FAMILIES);
  const listed = families.includes(value.family) ? families : [value.family, ...families];
  return (
    <Select
      label={label}
      value={value.family}
      choices={listed.map((family) => ({ value: family, label: family }))}
      onPick={(family) => family !== value.family && onPick({ family, source: { type: 'bundled' } })}
    />
  );
}
