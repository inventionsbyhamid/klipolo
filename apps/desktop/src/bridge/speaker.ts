import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { FormatKind } from '../bindings/FormatKind';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import type { SpeakerEstimate } from '../bindings/SpeakerEstimate';
import type { SpeakerProgress } from '../bindings/SpeakerProgress';
import { call } from './call';

/**
 * Follow the speaker (`crates/klipolo-app/src/commands/speaker.rs`): what it costs for a clip in a format before it
 * runs, the run itself (its change comes back as `project_apply`'s does, already in the history), and its progress.
 * A failure is an Error with one plain sentence.
 */
export const speakerBridge = {
  /** The price, free for footage tracked before, and why it can't run now. */
  estimate: (clipId: string, format: FormatKind) =>
    call<SpeakerEstimate>('speaker_estimate', { clipId, format }),
  /** Tracks the speaker and makes the clip follow them in `format`, as one undo step. */
  follow: (clipId: string, format: FormatKind) => call<SnapshotPatch>('speaker_follow', { clipId, format }),
  /** How far a follow is, 0 to 100. */
  onProgress: (handler: (progress: SpeakerProgress) => void): Promise<UnlistenFn> =>
    listen<SpeakerProgress>('speaker://progress', (event) => handler(event.payload)),
};
