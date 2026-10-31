import { useLingui } from '@lingui/react/macro';
import { useRef } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Framing } from '../../bindings/Framing';
import type { FramingMode } from '../../bindings/FramingMode';
import { Segmented } from '../../design/Segmented';
import { editClip } from '../../state/latest';
import { framingPatch } from './clipOps';
import { Field } from './fields';
import { useClipFraming } from './useClipFraming';

/** What a framing change starts from before Rust has answered (the centre). */
const CENTRE: Framing = { mode: 'fill', focusX: 0.5, focusY: 0.5 };

/**
 * A main clip's framing in the format shown: Fit or Fill as Rust frames it (`useClipFraming`: its own, else the
 * model's default). Each change is one undo step built at its turn on the clip's framing as the project has it then,
 * so another format's change still on its way stays; without a framing of its own in this format, Rust's. A mode
 * change keeps the rest, follow too (Fit ignores it; Fill follows again).
 */
export function FramingField({ clip, format }: { clip: Clip; format: FormatKind }) {
  const { t } = useLingui();
  const rust = useClipFraming(clip.id);
  const rustNow = useRef(rust);
  rustNow.current = rust;
  // What it draws in this format: its own framing, else Rust's answer (none shows as chosen until it comes).
  const framing = clip.framing[format] ?? rust?.framing[format] ?? null;
  /** The framing a change builds on: the clip's in this format as the project has it then, else Rust's. */
  const base = (now: Clip): Framing => now.framing[format] ?? rustNow.current?.framing[format] ?? CENTRE;
  const setMode = (mode: FramingMode) =>
    void editClip(mode === 'fit' ? 'clip.framing.fit.undo' : 'clip.framing.fill.undo', clip, (now) =>
      framingPatch(now, format, { ...base(now), mode }),
    );
  const label = t({ id: 'clip.framing', message: 'Framing' });
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
    </>
  );
}
