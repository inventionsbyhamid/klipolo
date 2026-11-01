import { t } from '@lingui/core/macro';
import type { Project } from '../../bindings/Project';
import type { Recording } from '../../bindings/Recording';
import { cameraSource, primarySource, recordingDurationUs } from '../../lib/recording';
import { formatRowLength } from '../../lib/time';
import { recordingName } from '../../lib/timeline';

/** A recording as the Library lists it: one row for the whole take. */
export interface RecordingRow {
  /** The recording's id. */
  id: string;
  /** "Recording 1", as the timeline names its clips. */
  name: string;
  durationUs: number;
  /** Its length and what it holds: "1:00, screen, mic and camera". */
  meta: string;
}

/**
 * What a take holds, after its length: its main footage (a screen, or a camera on its own), then a mic, then a camera
 * bubble. In the active language.
 */
function sourcesSaid(recording: Recording, duration: string): string {
  const mic = recording.sources.some((s) => s.role === 'mic');
  if (primarySource(recording)?.role === 'camera')
    return mic
      ? t({
          id: 'library.recording.cameraMic',
          message: `${duration}, camera and mic`,
          comment: 'A recording’s row; {duration} is its length, e.g. 1:00',
        })
      : t({
          id: 'library.recording.camera',
          message: `${duration}, camera`,
          comment: 'A recording’s row; {duration} is its length, e.g. 1:00',
        });
  const bubble = cameraSource(recording) !== undefined;
  if (mic && bubble)
    return t({
      id: 'library.recording.screenMicCamera',
      message: `${duration}, screen, mic and camera`,
      comment: 'A recording’s row; {duration} is its length, e.g. 1:00',
    });
  if (mic)
    return t({
      id: 'library.recording.screenMic',
      message: `${duration}, screen and mic`,
      comment: 'A recording’s row; {duration} is its length, e.g. 1:00',
    });
  if (bubble)
    return t({
      id: 'library.recording.screenCamera',
      message: `${duration}, screen and camera`,
      comment: 'A recording’s row; {duration} is its length, e.g. 1:00',
    });
  return t({
    id: 'library.recording.screen',
    message: `${duration}, screen`,
    comment: 'A recording’s row; {duration} is its length, e.g. 1:00',
  });
}

/** The project's recordings, each as one Library row, in the order they were made. */
export function recordingRows(project: Pick<Project, 'recordings'>): RecordingRow[] {
  return project.recordings.map((recording) => {
    const durationUs = recordingDurationUs(recording);
    return {
      id: recording.id,
      name: recordingName(project, recording.id),
      durationUs,
      meta: sourcesSaid(recording, formatRowLength(durationUs)),
    };
  });
}

const filesOf = new WeakMap<Recording[], Set<string>>();

/**
 * The files of every recording (the asset ids in any of their sources' runs): the Library shows each take as one row,
 * so it lists none of them on its own. Worked out once per recordings list.
 */
export function recordingFiles(project: Pick<Project, 'recordings'>): Set<string> {
  let files = filesOf.get(project.recordings);
  if (files) return files;
  files = new Set(project.recordings.flatMap((r) => r.sources.flatMap((s) => s.runs.map((run) => run.assetId))));
  filesOf.set(project.recordings, files);
  return files;
}
