import { useLingui } from '@lingui/react/macro';
import { useId } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { ClipPatch } from '../../bindings/ClipPatch';
import { Segmented } from '../../design/Segmented';
import { editClip } from '../../state/latest';
import { useProject } from '../../state/project';
import { clipFades, type FadeEnd } from './clipOps';
import { Field } from './fields';

/** A fade's length as the cards show it: "0.5 s", "1 s". */
const seconds = (us: number) => `${(us / 1_000_000).toFixed(1).replace(/\.0$/, '')} s`;

/**
 * A clip's fades (the clip card): Fade in from black and Fade out to black, None, 0.5 s or 1 s each, one undo step per
 * change. A length that would make the two longer than the clip is off, and so is an end where a transition blends the
 * cut (it replaces the fade there); a note under them says why, and the options it explains point to it.
 */
export function ClipFadesField({ clip }: { clip: Clip }) {
  const { t } = useLingui();
  const main = useProject((s) => s.snapshot?.project.timeline.main);
  const tooLongId = useId();
  const blendsId = useId();
  if (!main) return null;
  const fades = clipFades(main, clip);
  const none = t({ id: 'clip.fades.none', message: 'None', comment: 'No fade' });
  const row = (end: FadeEnd, label: string, patch: (us: number) => ClipPatch) => (
    <Field label={label} stack>
      <Segmented
        label={label}
        value={String(end.us)}
        options={end.choices.map(({ us, tooLong }) => ({
          value: String(us),
          label: us === 0 ? none : seconds(us),
          disabled: end.blends || tooLong,
          describedBy: end.blends ? blendsId : tooLong ? tooLongId : undefined,
        }))}
        fill
        onChange={(v) => void editClip('clip.fades.undo', clip, patch(Number(v)))}
      />
    </Field>
  );
  const tooLong = [fades.in, fades.out].some((end) => !end.blends && end.choices.some((c) => c.tooLong));
  return (
    <>
      {row(fades.in, t({ id: 'clip.fades.in', message: 'Fade in' }), (fadeInUs) => ({ fadeInUs }))}
      {row(fades.out, t({ id: 'clip.fades.out', message: 'Fade out' }), (fadeOutUs) => ({ fadeOutUs }))}
      {tooLong && (
        <p id={tooLongId} className="field-note">
          {t({ id: 'clip.fades.tooLong', message: 'Longer than the clip.' })}
        </p>
      )}
      {(fades.in.blends || fades.out.blends) && (
        <p id={blendsId} className="field-note">
          {t({
            id: 'clip.fades.transition',
            message: 'A transition replaces the fade here.',
            comment: 'Why a fade can’t be set: a transition at that cut blends the clips instead',
          })}
        </p>
      )}
    </>
  );
}
