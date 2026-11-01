import { useLingui } from '@lingui/react/macro';
import { useId } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { Recording } from '../../bindings/Recording';
import type { RecordingSource } from '../../bindings/RecordingSource';
import { Toggle } from '../../design/Toggle';
import { labelText, type LabelId } from '../../lib/labels';
import { mixFor } from '../../lib/recording';
import { keyBurst } from '../../state/keyBurst';
import { changesNothing, fillGapsOps, personMuted } from '../timeline/keys/sourceMix';
import { commit, editable } from '../timeline/keys/timelineActions';

/** Recording `recordingId`'s camera sound fills its gaps, or leaves them silent: one undo step (`fillGapsOps`). */
function setFillGaps(recordingId: string, on: boolean) {
  keyBurst.after(() => {
    const project = editable();
    const r = project?.recordings.find((x) => x.id === recordingId);
    if (!project || !r) return;
    const ops = fillGapsOps(project, r, on);
    if (ops.length === 0 || changesNothing(project, ops)) return;
    const label: LabelId = on ? 'undo.source.fillGaps' : 'undo.source.noFillGaps';
    commit(label, ops, labelText(label));
  });
}

/**
 * The camera sound's "Fill gaps" switch, on its row in Sounds: on, the camera's own sound plays where the separately
 * recorded sound has nothing (a recorder that started late or stopped early); off, those stretches stay silent. It
 * acts on the whole recording (`fillGapsOps`), and shows how this clip plays it. The camera-sound lane doesn't mark
 * the stretches it fills. `said`: the id of the row's own line when it already says what filling does ("Plays only
 * where your recording has nothing."): the switch is then described by it and shows no hint of its own, so the row
 * says it once.
 */
export function FillGapsSwitch({
  clip,
  recording,
  source,
  name,
  said,
}: {
  clip: Clip;
  recording: Recording;
  source: RecordingSource;
  name: string;
  said?: string;
}) {
  const { t } = useLingui();
  const id = useId();
  const hint = useId();
  const mix = mixFor(clip, source);
  const on = mix.fillsGaps === true;
  // The person muted it in this clip: the switch can't fill here, and says why.
  const mutedHere = personMuted(mix);
  return (
    <div className="sound-switch">
      <div className="sound-switch-row">
        <label htmlFor={id} className="sound-switch-label">
          {t({
            id: 'source.fillGaps',
            message: 'Fill gaps',
            comment: "Switch on the camera sound's row: the camera's sound plays where the recorder has nothing",
          })}
        </label>
        <Toggle
          id={id}
          aria-label={t({
            id: 'source.fillGaps.name',
            message: `${name}: fill gaps`,
            comment: 'Screen-reader name of the Fill gaps switch; {name} is e.g. Camera sound',
          })}
          aria-describedby={mutedHere ? hint : (said ?? hint)}
          checked={on}
          onCheckedChange={(next) => setFillGaps(recording.id, next)}
        />
      </div>
      {(mutedHere || !said) && (
        <p id={hint} className="field-note">
          {mutedHere
            ? t({
                id: 'source.fillGaps.mutedHere',
                message: 'You muted the camera’s sound in this clip, so it stays silent here, gaps included.',
                comment: 'Under the Fill gaps switch when the person muted the camera sound in the selected clip',
              })
            : t({
                id: 'source.fillGaps.hint',
                message: 'Where your recording has nothing, the camera’s sound plays.',
                comment: 'Under the Fill gaps switch of the camera sound',
              })}
        </p>
      )}
    </div>
  );
}
