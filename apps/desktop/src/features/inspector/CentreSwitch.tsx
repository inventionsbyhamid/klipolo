import { useLingui } from '@lingui/react/macro';
import { useEffect, useId, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import type { Clip } from '../../bindings/Clip';
import type { ErrorParam } from '../../bindings/ErrorParam';
import type { RecordingSource } from '../../bindings/RecordingSource';
import { Toggle } from '../../design/Toggle';
import { mixFor } from '../../lib/recording';
import { useMedia } from '../../state/media';
import { useProject } from '../../state/project';
import { setKindCentred } from '../timeline/keys/sourceActions';
import { peaksInfoOf } from '../timeline/media/peaksCache';

/**
 * Whether any of `source`'s files has two channels, as its sound cache's header says (the timeline's waveforms read
 * the same, so it is usually loaded already): false until the cache is built. O(runs).
 */
function useTwoChannels(source: RecordingSource): boolean {
  const paths = useProject(
    useShallow((s) =>
      source.runs.map((run) => s.snapshot?.project.assets.find((a) => a.id === run.assetId)?.path ?? ''),
    ),
  );
  const versions = useMedia(useShallow((s) => paths.map((path) => s.versions[path] ?? 0)));
  const [two, setTwo] = useState(false);
  useEffect(() => {
    let live = true;
    const infos = paths.map((path, i) => (path ? peaksInfoOf(path, versions[i]).catch(() => null) : null));
    void Promise.all(infos).then((found) => {
      if (live) setTwo(found.some((info) => (info?.channels ?? 0) >= 2));
    });
    return () => {
      live = false;
    };
  }, [paths, versions]);
  return two;
}

/**
 * A recorded sound's Centre switch, shown only for a sound with two channels: a mic on one input of a two-input
 * interface records [voice, silence] and is heard on one side. On, it plays in both ears from the side that carries
 * it, in every clip (`setKindCentred`); off, as recorded. It shows how this clip plays the sound, also while it plays
 * centred whatever the cache says, so it can always be turned off. Where the sound plays from its own track in this
 * clip (`own`) it is inert, with the row's reason, and stays focusable like the row's mute. It centres the recordings'
 * `nth` source of its role alone ("Mic 2", a camera's own sound), named `param` in the undo label.
 */
export function CentreSwitch({
  clip,
  source,
  nth,
  name,
  param,
  own,
}: {
  clip: Clip;
  source: RecordingSource;
  nth: number;
  name: string;
  param: ErrorParam;
  own: boolean;
}) {
  const { t } = useLingui();
  const id = useId();
  const hint = useId();
  const centred = mixFor(clip, source).centreFrom != null;
  const two = useTwoChannels(source);
  if (!two && !centred) return null;
  return (
    <div className="sound-switch">
      <div className="sound-switch-row">
        <label htmlFor={id} className="sound-switch-label">
          {t({ id: 'source.centre', message: 'Centre', comment: 'Switch: a sound recorded on one side plays in both ears' })}
        </label>
        <Toggle
          id={id}
          aria-label={t({
            id: 'source.centre.name',
            message: `${name}: centre`,
            comment: 'Screen-reader name of the Centre switch; {name} is a recorded sound, e.g. Mic',
          })}
          aria-describedby={hint}
          checked={centred}
          aria-disabled={own || undefined}
          onCheckedChange={(on) => {
            if (!own) void setKindCentred(source.role, on, nth, param);
          }}
        />
      </div>
      <p id={hint} className="field-note">
        {t({
          id: 'source.centre.hint',
          message: 'Plays a sound recorded on one side in both ears, in every clip.',
          comment: 'Under the Centre switch of a recorded sound',
        })}
      </p>
    </div>
  );
}
