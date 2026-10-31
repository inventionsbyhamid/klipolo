import type { Label } from '../../../lib/labels';

/** The commit queue's senders live in `state/commit`; re-exported for the tools that import them from here. */
export { commitLatest, commitToggle, queueCommit, type QueuedCommit } from '../../../state/commit';

/** The op builders live in `lib/ops`; re-exported for the tools that import them from here. */
export { clipOp, itemOp } from '../../../lib/ops';

/** A mute toggle's undo label: `muting` a "sound" is "Mute sound", else "Unmute sound" (`noun` picks the word). */
export const muteLabel = (muting: boolean, noun: 'clip' | 'video' | 'sound'): Label => ({
  id: muting ? 'volume.mute.undo' : 'volume.unmute.undo',
  params: { noun },
});
