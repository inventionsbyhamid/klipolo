import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';

/*
 * Following the speaker is paid for (the footage tracked, and whose voice is whose): once a follow ends, landed or
 * failed, the credit the top bar and Settings show is asked for again. Who is kept is made at no charge.
 */

vi.mock('../../ai/aiStatus', async (real) => ({ ...(await real<object>()), creditMoved: vi.fn() }));
vi.mock('../../bridge/speaker', () => ({
  speakerBridge: {
    estimate: vi.fn(),
    follow: vi.fn(),
    keep: vi.fn(),
    onProgress: vi.fn(() => Promise.resolve(() => undefined)),
  },
}));
const { creditMoved } = await import('../../ai/aiStatus');
const { speakerBridge } = await import('../../bridge/speaker');
const { useProject } = await import('../../state/project');
const { followSpeaker, keepInFrame } = await import('./speakerRuns');

const adopt = vi.fn((_change: SnapshotPatch) => Promise.resolve(true));

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ adopt });
});

describe('the credit after a follow', () => {
  it('is asked for again once a follow lands', async () => {
    vi.mocked(speakerBridge.follow).mockResolvedValue({ change: { baseRevision: 3 } as SnapshotPatch });
    expect(await followSpeaker('b', 'portrait')).toBe(true);
    expect(creditMoved).toHaveBeenCalledTimes(1);
  });

  it('is asked for again when a follow fails: what was tracked before it failed was paid for', async () => {
    vi.mocked(speakerBridge.follow).mockRejectedValue(new Error('Meta is busy right now. Try again in a moment.'));
    expect(await followSpeaker('b', 'portrait')).toBe(false);
    expect(creditMoved).toHaveBeenCalledTimes(1);
  });

  it('is not asked for when who is kept changes, which costs nothing', async () => {
    vi.mocked(speakerBridge.keep).mockResolvedValue({ baseRevision: 3 } as SnapshotPatch);
    expect(await keepInFrame('b', 'portrait', { kind: 'speaker' })).toBe(true);
    expect(creditMoved).not.toHaveBeenCalled();
  });
});
