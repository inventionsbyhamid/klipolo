import { useLingui } from '@lingui/react/macro';
import { useRef } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Framing } from '../../bindings/Framing';
import type { FramingMode } from '../../bindings/FramingMode';
import { Segmented } from '../../design/Segmented';
import { clipOp } from '../../lib/ops';
import { clipNow, editClip } from '../../state/latest';
import { framingPatch } from './clipOps';
import { Field } from './fields';
import { ToolSlider } from './ToolSlider';
import { useClipFraming } from './useClipFraming';

/** What a framing change starts from before Rust has answered (the centre). */
const CENTRE: Framing = { mode: 'fill', focusX: 0.5, focusY: 0.5 };
const percent = (v: number) => `${Math.round(v * 100)}%`;
/** A Fill crop at least this share of the footage on a side leaves nothing there for a focus to move. */
const WHOLE = 0.999;

/**
 * A main clip's framing in the format shown: Fit or Fill as Rust frames it (`useClipFraming`: its own, else the
 * model's default), and while Fill crops from a focus of its own (not following the cursor), the Fill focus: which part
 * of the video stays in view (0.5 is the centre), the way Rust says Fill crops there (`fillCrop`): Across for a video
 * wider than the frame, Down for a taller one, none when it fits. Each change is one undo step built at its turn
 * on the clip's framing as the project has it then, so another format's change still on its way stays; without a
 * framing of its own in this format, Rust's. A mode change keeps the rest, follow too (Fit ignores it; Fill follows
 * again). A focus drag is dry-run in the preview and commits once on release.
 */
export function FramingField({ clip, format }: { clip: Clip; format: FormatKind }) {
  const { t } = useLingui();
  const rust = useClipFraming(clip.id);
  const rustNow = useRef(rust);
  rustNow.current = rust;
  // What it draws in this format: its own framing, else Rust's answer (none shows as chosen until it comes).
  const framing = clip.framing[format] ?? rust?.framing[format] ?? null;
  const crop = rust?.fillCrop[format];
  const cropsAcross = !!crop && crop.w < WHOLE;
  const cropsDown = !!crop && crop.h < WHOLE;
  /** The framing a change builds on: the clip's in this format as the project has it then, else Rust's. */
  const base = (now: Clip): Framing => now.framing[format] ?? rustNow.current?.framing[format] ?? CENTRE;
  const setMode = (mode: FramingMode) =>
    void editClip(mode === 'fit' ? 'clip.framing.fit.undo' : 'clip.framing.fill.undo', clip, (now) =>
      framingPatch(now, format, { ...base(now), mode }),
    );
  const focusOps = (axis: 'focusX' | 'focusY') => (value: number) => {
    const now = clipNow(clip.id) ?? clip;
    return [clipOp(clip, framingPatch(now, format, { ...base(now), [axis]: value }))];
  };
  const label = t({ id: 'clip.framing', message: 'Framing' });
  const across = t({ id: 'clip.focus.across', message: 'Across', comment: 'The Fill focus, left to right' });
  const down = t({ id: 'clip.focus.down', message: 'Down', comment: 'The Fill focus, top to bottom' });
  return (
    <>
      <Field label={label}>
        <Segmented
          label={label}
          value={framing?.mode ?? null}
          options={[
            {
              value: 'fit',
              label: t({ id: 'clip.framing.fit', message: 'Fit', comment: 'Show the whole video inside the frame' }),
              tooltip: t({ id: 'clip.framing.fit.tip', message: 'Show the whole video' }),
            },
            {
              value: 'fill',
              label: t({ id: 'clip.framing.fill', message: 'Fill', comment: 'Fill the frame with the video' }),
              tooltip: t({ id: 'clip.framing.fill.tip', message: 'Fill the frame, cropping the edges' }),
            },
          ]}
          fill
          onChange={setMode}
        />
      </Field>
      {framing?.mode === 'fill' && !framing.follow && (cropsAcross || cropsDown) && (
        <Field
          label={t({ id: 'clip.focus', message: 'Fill focus', comment: 'Which part of a filled video stays in view' })}
          stack
        >
          {cropsAcross && (
            <Field label={across}>
              <ToolSlider
                label={t({ id: 'clip.focus.across.named', message: 'Focus across' })}
                commitLabel="clip.framing.focus.undo"
                value={framing.focusX}
                min={0}
                max={1}
                step={0.01}
                format={percent}
                ops={focusOps('focusX')}
              />
            </Field>
          )}
          {cropsDown && (
            <Field label={down}>
              <ToolSlider
                label={t({ id: 'clip.focus.down.named', message: 'Focus down' })}
                commitLabel="clip.framing.focus.undo"
                value={framing.focusY}
                min={0}
                max={1}
                step={0.01}
                format={percent}
                ops={focusOps('focusY')}
              />
            </Field>
          )}
        </Field>
      )}
    </>
  );
}
