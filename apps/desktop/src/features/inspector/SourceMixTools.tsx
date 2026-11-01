import { useLingui } from '@lingui/react/macro';
import { Volume2, VolumeX } from 'lucide-react';
import { useId } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { Recording } from '../../bindings/Recording';
import type { RecordingSource } from '../../bindings/RecordingSource';
import { Button } from '../../design/Button';
import { IconButton } from '../../design/IconButton';
import { cameraSource, isVideoRole, mixFor, nthOfRole, soundSources } from '../../lib/recording';
import { soundName, soundParam, sourceName } from '../../lib/sourceNames';
import { useProject } from '../../state/project';
import {
  clipMixOps,
  clipMuteGapsName,
  clipMuteName,
  ownTrackReason,
  setClipSourceMuted,
  soundUndoLabel,
} from '../timeline/keys/sourceActions';
import { onOwnTrack, silent } from '../timeline/keys/sourceMix';
import { CentreSwitch } from './CentreSwitch';
import { FillGapsSwitch } from './FillGapsSwitch';
import { SourceSync } from './SourceSync';
import { ToolSlider } from './ToolSlider';

const percent = (v: number) => `${Math.round(v * 100)}%`;
const icon = (Icon: typeof Volume2) => <Icon size={18} strokeWidth={1.8} aria-hidden />;

/**
 * A recording clip's Sounds, in its card: each sound it can be heard from (`soundSources`: a camera's own sound too,
 * where its files carry one), each with its mute and volume in this clip (the clip's own mix; off, with why, where the
 * sound plays from its own track in the clip), its Centre switch where it has two channels (every clip), a camera
 * sound's Fill gaps switch, and a recorded sound's sync against the picture for the whole recording (`SourceSync`);
 * then the camera's sync. Each change is one undo step. Each row is `data-source-row` (its source's id).
 */
export function SourceMixTools({ clip, recording }: { clip: Clip; recording: Recording }) {
  const { t } = useLingui();
  const head = useId();
  const assets = useProject((s) => s.snapshot?.project.assets);
  const sounds = assets ? soundSources({ assets }, recording) : [];
  const camera = cameraSource(recording);
  if (sounds.length === 0 && !camera) return null;
  return (
    <div className="field field-col" role="group" aria-labelledby={head} data-source-mix>
      <span id={head} className="field-label">
        {t({
          id: 'source.tools',
          message: 'Sounds',
          comment: "The clip card's part for a recording clip's recorded sounds (mic, computer sound)",
        })}
      </span>
      <ul className="sound-rows">
        {sounds.map((s) => (
          <SourceRow key={s.id} clip={clip} recording={recording} source={s} />
        ))}
        {camera && (
          <li className="sound-row" data-source-row={camera.id}>
            <span className="sound-name">{sourceName(camera)}</span>
            <SourceSync recordingId={recording.id} source={camera} name={sourceName(camera)} />
          </li>
        )}
      </ul>
    </div>
  );
}

function SourceRow({ clip, recording, source }: { clip: Clip; recording: Recording; source: RecordingSource }) {
  const { t } = useLingui();
  const hint = useId();
  const status = useId();
  const recordingId = recording.id;
  const name = soundName(recording, source);
  const param = soundParam(recording, source);
  // A picture's own sound (a camera's): kept in case; it plays with its picture, so no sync.
  const picture = isVideoRole(source.role);
  const mix = mixFor(clip, source);
  const mute = clipMuteName(name, mix.muted);
  // Muted but still filling the gaps: the row says so in words, once, with two named buttons, Unmute (first) and Mute,
  // its fill off too; Mute is the one verb the undo history uses for it.
  const filling = mix.muted && !silent(mix);
  // Where it plays from its own track in this clip, separating set its mix here: changing it would play it twice.
  const own = useProject((state) => {
    const project = state.snapshot?.project;
    return project
      ? onOwnTrack(
          project,
          project.timeline.main.findIndex((c) => c.id === clip.id),
          source,
        )
      : false;
  });
  const reason = own ? ownTrackReason(name) : undefined;
  return (
    <li className="sound-row" data-source-row={source.id}>
      <div className="sound-row-head">
        <span className="sound-name">{name}</span>
        {filling ? (
          <>
            <Button
              size="sm"
              aria-label={mute}
              aria-describedby={status}
              aria-disabled={own || undefined}
              title={reason}
              onClick={() => !own && setClipSourceMuted(clip.id, recordingId, source.id, false)}
            >
              {t({ id: 'source.button.unmute', message: 'Unmute', comment: "Button on a sound's row in Sounds" })}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              aria-label={clipMuteGapsName(name)}
              aria-describedby={status}
              aria-disabled={own || undefined}
              title={reason}
              onClick={() => !own && setClipSourceMuted(clip.id, recordingId, source.id, true)}
            >
              {t({
                id: 'source.button.muteGaps',
                message: 'Mute',
                comment:
                  "Button on the camera sound's row in Sounds: no sound in this clip, the gaps included (Unmute is beside it)",
              })}
            </Button>
          </>
        ) : (
          // Its name says what it does next, so it has no pressed state as well.
          <IconButton
            className="icon-btn-sm"
            icon={icon(mix.muted ? VolumeX : Volume2)}
            label={mute}
            tooltip={reason}
            aria-disabled={own || undefined}
            aria-describedby={picture ? hint : undefined}
            onClick={() => !own && setClipSourceMuted(clip.id, recordingId, source.id, !mix.muted)}
          />
        )}
      </div>
      {filling && (
        <p id={status} className="field-note">
          {t({
            id: 'source.filling.status',
            message: 'Plays only where your recording has nothing.',
            comment: "On the camera sound's row in Sounds: muted, it fills the stretches the recorder didn't record",
          })}
        </p>
      )}
      {picture && (
        <p id={hint} className="field-note">
          {source.role === 'camera'
            ? t({
                id: 'source.cameraSound.hint',
                message: 'The sound the camera recorded. Kept in case you need it.',
                comment: "Under the Camera sound row in Sounds: a camera's own sound",
              })
            : t({
                id: 'source.cameraSound.hintVideo',
                message: 'The sound this video recorded. Kept in case you need it.',
                comment: "Under a screen or game file's own sound in Sounds",
              })}
        </p>
      )}
      <ToolSlider
        label={t({
          id: 'source.volume',
          message: `${name} volume`,
          comment: "A recorded sound's volume; {name} is e.g. Mic or Computer sound",
        })}
        disabled={own}
        commitLabel={soundUndoLabel('undo.source.volume', param)}
        value={mix.volume}
        min={0}
        max={2}
        step={0.01}
        format={percent}
        ops={(volume) => clipMixOps(clip.id, recordingId, source.id, { volume })}
        preview={false}
      />
      {own && <p className="field-note">{reason}</p>}
      <CentreSwitch
        clip={clip}
        source={source}
        nth={nthOfRole(recording, source)}
        name={name}
        param={param}
        own={own}
      />
      {picture && (
        <FillGapsSwitch
          clip={clip}
          recording={recording}
          source={source}
          name={name}
          said={filling ? status : undefined}
        />
      )}
      {!picture && <SourceSync recordingId={recordingId} source={source} name={name} />}
    </li>
  );
}
