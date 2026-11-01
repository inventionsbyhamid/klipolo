import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { Project } from '../../bindings/Project';
import type { RecordingSource } from '../../bindings/RecordingSource';
import { codedMessage } from '../../bridge/call';
import { labelText } from '../../lib/labels';
import { isVideoRole, mixFor, runInClip, soundSources } from '../../lib/recording';
import { soundName, soundParam } from '../../lib/sourceNames';
import { findClip, itemEndUs } from '../../lib/timeline';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { searchWords, type Command } from '../command-palette/command';
import {
  clipMuteGapsName,
  clipMuteName,
  ownTrackReason,
  separateSource,
  setClipSourceMuted,
  soundUndoLabel,
} from './keys/sourceActions';
import { onOwnTrack, silent } from './keys/sourceMix';
import { mainIndex } from './model/index';

/** Command ids for one sound of the selected recording clip: its mute in this clip, and Separate. */
export const sourceMuteId = (sourceId: string) => `source-mute:${sourceId}`;
export const sourceSeparateId = (sourceId: string) => `source-separate:${sourceId}`;
/** A filling camera sound's "Mute ... in this clip" beside its Unmute: a mute item, so the block menu lists it with them. */
export const sourceMuteGapsId = (sourceId: string) => `source-mute:${sourceId}:gaps`;

/** Whether some of `s`'s sound inside clip `i` already plays from a layer of its own (what `detachSource` refuses). */
function separated(project: Project, i: number, s: RecordingSource): boolean {
  const { starts, ends } = mainIndex(project.timeline.main);
  const files = new Set(s.runs.map((run) => run.assetId));
  return project.timeline.tracks.some((track) =>
    track.items.some(
      (item) =>
        item.content.type === 'audio' &&
        files.has(item.content.assetId) &&
        item.startUs < ends[i] &&
        itemEndUs(item) > starts[i],
    ),
  );
}

/** The model's own sentence for separating a sped-up clip's sound (`model.source.detachSpeed`). */
const detachSpeed = () => codedMessage({ code: 'model.source.detachSpeed', params: {} });

/**
 * The selected recording clip's sounds as commands: "Mute {sound} in this clip" or "Unmute ..." (the clip's own mix,
 * `setClipSourceMix`) and "Separate {sound}" (`detachSource`). The block menu lists them (it selects its block
 * first), and so does the palette while such a clip is selected.
 */
export function useSourceCommands(): Command[] {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  return useMemo(() => {
    const clip: Clip | null = project && selectedId ? findClip(project, selectedId) : null;
    const source = clip?.source;
    const rec = source?.type === 'recording' ? project!.recordings.find((r) => r.id === source.id) : undefined;
    if (!project || !clip || !rec) return [];
    const i = project.timeline.main.indexOf(clip);
    const start = mainIndex(project.timeline.main).starts[i];
    // Each sound the clip can be heard from (`soundSources`: a camera's own sound too), named as its row in Sounds
    // names it ("Camera sound", "Mic 2").
    return soundSources(project, rec).flatMap((s): Command[] => {
      const name = soundName(rec, s);
      // Only where it recorded something inside this clip.
      const recorded = s.runs.some((run) => runInClip(clip, start, s, run) !== null);
      const mix = mixFor(clip, s);
      const muted = mix.muted;
      // Where it plays from its own track, separating set its mix here: unmuting it would play it twice.
      const own = onOwnTrack(project, i, s);
      const sped = clip.speed !== 1;
      const mute: Command = {
        id: sourceMuteId(s.id),
        action: 'setClipSourceMix',
        group: 'edit',
        label: clipMuteName(name, muted),
        keywords: searchWords(
          t({
            id: 'command.sourceMute.keywords',
            message: 'audio, sound, silence, mic, game sound',
            comment: 'Comma-separated search words for Mute (a recorded sound) in this clip',
          }),
        ),
        enabled: recorded && !own,
        reason: own ? ownTrackReason(name) : undefined,
        run: () => setClipSourceMuted(clip.id, rec.id, s.id),
      };
      // Muted but still filling the gaps, a camera's sound has "Mute ... in this clip (the gaps too)" beside
      // "Unmute ...": it mutes it there, its fill off.
      const gapsToo: Command[] =
        muted && !silent(mix)
          ? [
              {
                ...mute,
                id: sourceMuteGapsId(s.id),
                label: clipMuteGapsName(name),
                run: () => setClipSourceMuted(clip.id, rec.id, s.id, true),
              },
            ]
          : [];
      // A picture's own sound plays with its picture: the camera has Separate camera.
      if (isVideoRole(s.role)) return [mute, ...gapsToo];
      return [
        mute,
        {
          id: sourceSeparateId(s.id),
          action: 'detachSource',
          group: 'edit',
          label: labelText(soundUndoLabel('undo.source.separate', soundParam(rec, s))),
          keywords: searchWords(
            t({
              id: 'command.sourceSeparate.keywords',
              message: 'detach, own track, split audio, layer',
              comment: 'Comma-separated search words for Separate (a recorded sound)',
            }),
          ),
          // The model refuses a sped-up clip's: off, with its sentence.
          enabled: recorded && !sped && !separated(project, i, s),
          reason: sped ? detachSpeed() : undefined,
          run: () => separateSource(clip.id, rec.id, s.id),
        },
      ];
    });
  }, [t, project, selectedId]);
}
