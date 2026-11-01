import { t } from '@lingui/core/macro';
import type { Clip } from '../../../bindings/Clip';
import type { ErrorParam } from '../../../bindings/ErrorParam';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { RecordingSource } from '../../../bindings/RecordingSource';
import type { SourceMix } from '../../../bindings/SourceMix';
import type { SourceRole } from '../../../bindings/SourceRole';
import { media } from '../../../bridge/media';
import { deepEqual } from '../../../lib/equal';
import { labelText, type Label, type LabelId } from '../../../lib/labels';
import { mixFor } from '../../../lib/recording';
import { soundParam, sourceParam } from '../../../lib/sourceNames';
import { findClip } from '../../../lib/timeline';
import { keepAtClose } from '../../../state/commitQueue';
import { keyBurst } from '../../../state/keyBurst';
import { useProject } from '../../../state/project';
import {
  changesNothing,
  kindCentreOps,
  kindMuteOps,
  kindVolumeOps,
  mutedMix,
  onOwnTrack,
  silent,
  sourceOfKind,
  type ChannelPeaks,
} from './sourceMix';
import { commit, editable } from './timelineActions';

/*
 * What a recording's sounds and markers do: a source lane's header (a kind of sound in every recording: `sourceMix`),
 * a clip's menu, the clip card's Sounds and the marker flags. Each is built from the project when it runs (after a
 * change still settling has gone in: `keyBurst.after`), and is one undo step through `useProject.apply`, said in the
 * timeline's live region.
 */

/** The model's sync range (`model.recording.nudge`): 5 s either way. */
export const MAX_NUDGE_US = 5_000_000;

export function sourceOf(project: Project | null | undefined, recordingId: string, sourceId: string) {
  return project?.recordings.find((r) => r.id === recordingId)?.sources.find((s) => s.id === sourceId);
}

/** An undo label that names source `s`. */
export const sourceUndoLabel = (id: LabelId, s: RecordingSource): Label => ({
  id,
  params: { source: sourceParam(s) },
});

/** An undo label that names a sound by `name` (`soundParam`, or a lane's name: "Camera sound", "Mic 2"). */
export const soundUndoLabel = (id: LabelId, name: ErrorParam): Label => ({ id, params: { source: name } });

const muteLabel = (muted: boolean, name: ErrorParam) =>
  soundUndoLabel(muted ? 'undo.source.mute' : 'undo.source.unmute', name);

/** A clip's menu item and the clip card's toggle for `name` (a source's name) in one clip, in the language shown. */
export const clipMuteName = (name: string, muted: boolean) =>
  muted
    ? t({
        id: 'source.menu.unmute',
        message: `Unmute ${name} in this clip`,
        comment: "A clip's menu item and button; {name} is a recorded sound, e.g. Mic or Computer sound",
      })
    : t({
        id: 'source.menu.mute',
        message: `Mute ${name} in this clip`,
        comment: "A clip's menu item and button; {name} is a recorded sound, e.g. Mic or Computer sound",
      });

/**
 * Muting a camera's sound that only fills the gaps in one clip: the clip's menu item and the Sounds row's button,
 * saying that its gaps go silent too. Mute is the one verb the undo history uses for it.
 */
export const clipMuteGapsName = (name: string) =>
  t({
    id: 'source.menu.muteGaps',
    message: `Mute ${name} in this clip (the gaps too)`,
    comment:
      "A clip's menu item and button for the camera's sound, which plays only where the recorder has nothing; {name} is e.g. Camera sound",
  });

/** One step labelled `label`, said as its label. */
const step = (label: Label, ops: Op[]) => commit(label, ops, labelText(label));

const sourceOp = (recordingId: string, sourceId: string, patch: { nudgeUs: number }): Op => ({
  type: 'updateRecordingSource',
  recordingId,
  sourceId,
  patch,
});

/**
 * A lane header's Mute: its kind of sound (each recording's `nth` of `role`) in every recording (`kindMuteOps`): each
 * default and each clip's own setting, leaving clips whose sound plays from its own track as they are. `name` is the
 * lane's, for the undo label.
 */
export function setKindMuted(role: SourceRole, muted: boolean, nth = 0, name?: ErrorParam) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const ops = kindMuteOps(project, role, muted, nth);
    if (ops.length === 0 || changesNothing(project, ops)) return;
    step(muteLabel(muted, name ?? firstName(project, role, nth)), ops);
  });
}

/** How undo labels name a kind of sound when its lane gave no name: the first recording's `nth` source of `role`. */
function firstName(project: Project, role: SourceRole, nth: number): ErrorParam {
  for (const r of project.recordings) {
    const s = sourceOfKind(r, role, nth);
    if (s) return soundParam(r, s);
  }
  return '';
}

/** A lane header's volume (0 to 2) for its kind of sound everywhere, built as the change goes in. */
export function kindVolumeOpsNow(role: SourceRole, volume: number, nth = 0): Op[] {
  const project = useProject.getState().snapshot?.project;
  if (!project) return [];
  const ops = kindVolumeOps(project, role, volume, nth);
  return changesNothing(project, ops) ? [] : ops;
}

/**
 * How loud each file of every recording's sound of `role` is on each channel (a file nothing knows yet, or that fails,
 * is left out).
 */
async function channelPeaksOf(project: Project, role: SourceRole, nth: number): Promise<ChannelPeaks> {
  const ids = new Set(
    project.recordings.flatMap((r) => sourceOfKind(r, role, nth)?.runs.map((run) => run.assetId) ?? []),
  );
  const found = await Promise.all(
    [...ids].map(async (id): Promise<[string, number[]] | null> => {
      const path = project.assets.find((a) => a.id === id)?.path;
      const peaks = path ? await media.channelPeaks(path).catch(() => null) : null;
      return peaks ? [id, peaks] : null;
    }),
  );
  return new Map(found.filter((f) => f !== null));
}

/**
 * The Centre switch: a kind of sound (each recording's `nth` of `role`) in every recording played in both ears from the
 * side that carries it, or as recorded (`kindCentreOps`): each default and each clip's own setting, never a clip whose
 * sound plays from its own track. Which side comes from how loud each file is on each channel, read before the change
 * is built. It is kept for the close from the press: a close while that is read waits for it, in its turn, and says it
 * if it can't, never dropping it unsaid.
 */
export async function setKindCentred(role: SourceRole, centred: boolean, nth = 0, name?: ErrorParam) {
  const now = useProject.getState().snapshot?.project;
  const id = centred ? 'undo.source.centre' : 'undo.source.asRecorded';
  const label = (project: Project) => soundUndoLabel(id, name ?? firstName(project, role, nth));
  const hold = now ? keepAtClose(label(now)) : null;
  let peaks: ChannelPeaks;
  try {
    peaks = centred && now ? await channelPeaksOf(now, role, nth) : new Map();
  } catch (error) {
    hold?.release();
    throw error;
  }
  keyBurst.after(
    () => {
      try {
        // The close gave up on it (and said so): it never goes in after.
        if (hold?.dropped) return;
        const project = editable();
        if (!project) return;
        const ops = kindCentreOps(project, role, centred, peaks, nth);
        if (ops.length === 0 || changesNothing(project, ops)) return;
        step(label(project), ops);
      } finally {
        hold?.release();
      }
    },
    undefined,
    hold,
  );
}

/** Why a clip's own mute and volume of `name` are off: it plays from its own track there (separated). */
export const ownTrackReason = (name: string) =>
  t({
    id: 'source.ownTrack.here',
    message: `${name} is on its own track here.`,
    comment: "Why a recorded sound's mute and volume in this clip are off; {name} is e.g. Mic",
  });

/** A clip's own mix of `s` becoming `next`: none of its own when that is the source's default again. */
export function clipMixOp(clipId: string, s: RecordingSource, next: SourceMix): Op {
  return { type: 'setClipSourceMix', clipId, sourceId: s.id, mix: deepEqual(next, s.mix) ? null : next };
}

/** Whether clip `clip`'s sound of `s` plays from a layer of its own (its own mix there stays as separating left it). */
const separatedIn = (project: Project, clip: Clip, s: RecordingSource) =>
  onOwnTrack(project, project.timeline.main.indexOf(clip), s);

/**
 * A clip's mix of `s` with `patch`, as the project has them now (a slider's ops); none where `s` plays from its own
 * track in the clip.
 */
export function clipMixOps(
  clipId: string,
  recordingId: string,
  sourceId: string,
  patch: Partial<SourceMix>,
): Op[] {
  const project = useProject.getState().snapshot?.project;
  const clip = project ? findClip(project, clipId) : null;
  const s = sourceOf(project, recordingId, sourceId);
  if (!project || !clip || !s || separatedIn(project, clip, s)) return [];
  return [clipMixOp(clipId, s, { ...mixFor(clip, s), ...patch })];
}

/**
 * Mute or unmute `sourceId` in clip `clipId` alone (its own mix); with no `muted`, the other way from now. Muting a
 * camera's sound that only fills the gaps (muted, but not `silent`) silences it for real, its fill off. Never where the
 * sound plays from its own track in the clip: unmuting it there would play it twice.
 */
export function setClipSourceMuted(clipId: string, recordingId: string, sourceId: string, muted?: boolean) {
  keyBurst.after(() => {
    const project = editable();
    const clip = project ? findClip(project, clipId) : null;
    const r = project?.recordings.find((x) => x.id === recordingId);
    const s = r?.sources.find((x) => x.id === sourceId);
    if (!project || !clip || !r || !s || separatedIn(project, clip, s)) return;
    const mix = mixFor(clip, s);
    const on = muted ?? !mix.muted;
    if (on ? silent(mix) : !mix.muted) return;
    step(muteLabel(on, soundParam(r, s)), [clipMixOp(clipId, s, mutedMix(mix, on))]);
  });
}

/** Separate {source}: the clip's part of that sound becomes its own layer (`detachSource`). */
export function separateSource(clipId: string, recordingId: string, sourceId: string) {
  keyBurst.after(() => {
    const r = editable()?.recordings.find((x) => x.id === recordingId);
    const s = r?.sources.find((x) => x.id === sourceId);
    if (!r || !s) return;
    step(soundUndoLabel('undo.source.separate', soundParam(r, s)), [{ type: 'detachSource', clipId, sourceId }]);
  });
}

/** Moves source `sourceId` `deltaUs` later (earlier when negative) against the picture, within the model's 5 s. */
export function nudgeSource(recordingId: string, sourceId: string, deltaUs: number) {
  keyBurst.after(() => {
    const s = sourceOf(editable(), recordingId, sourceId);
    if (!s) return;
    const nudgeUs = Math.max(-MAX_NUDGE_US, Math.min(MAX_NUDGE_US, s.nudgeUs + deltaUs));
    if (nudgeUs === s.nudgeUs) return;
    step(sourceUndoLabel('undo.source.sync', s), [sourceOp(recordingId, sourceId, { nudgeUs })]);
  });
}

/** Removes marker `id` of recording `recordingId`: the person's own edit, so it goes in at once. */
export function removeMarker(recordingId: string, id: string, then?: () => void) {
  keyBurst.after(() => {
    const project = editable();
    if (!project?.recordings.some((r) => r.id === recordingId && r.markers.some((m) => m.id === id))) return;
    commit(
      'undo.marker.remove',
      [{ type: 'removeMarker', recordingId, id }],
      labelText('undo.marker.remove'),
      then,
    );
  });
}
