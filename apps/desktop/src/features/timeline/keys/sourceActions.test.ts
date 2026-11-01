import { beforeEach, describe, expect, it, vi } from 'vitest';
import { twoTakesSnapshot } from '../../../test/sounds';

vi.mock('../../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { bridge } = await import('../../../bridge/commands');
const { useProject } = await import('../../../state/project');
const actions = await import('./sourceActions');

/** Every committed batch: (label, params, ops). */
const applied = () =>
  vi.mocked(bridge.apply).mock.calls.map(([tx]) => ({ label: tx.label, params: tx.params, ops: tx.ops }));
const settle = () => vi.waitFor(() => expect(useProject.getState().pending).toBe(0));

beforeEach(() => {
  useProject.setState({ snapshot: twoTakesSnapshot(), pending: 0 });
  vi.mocked(bridge.apply)
    .mockReset()
    .mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [] },
      };
    });
});

describe("one clip's sounds", () => {
  it("mutes one sound in one clip alone, as the person's mute, in one step", async () => {
    actions.setClipSourceMuted('c2', 'r1', 'mic');
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(applied()).toEqual([
      {
        label: 'undo.source.mute',
        params: { source: { code: 'source.role.mic', params: {} } },
        ops: [
          {
            type: 'setClipSourceMix',
            clipId: 'c2',
            sourceId: 'mic',
            mix: { volume: 0.6, muted: true, effects: [], mutedByPerson: true },
          },
        ],
      },
    ]);
  });

  it("sets one sound's volume in one clip, dropping the clip's own setting when it is the default again", () => {
    expect(actions.clipMixOps('c3', 'r2', 'mic', { volume: 0.5 })).toEqual([
      { type: 'setClipSourceMix', clipId: 'c3', sourceId: 'mic', mix: { volume: 0.5, muted: false, effects: [] } },
    ]);
    expect(actions.clipMixOps('c2', 'r1', 'mic', { volume: 1 })).toEqual([
      { type: 'setClipSourceMix', clipId: 'c2', sourceId: 'mic', mix: null },
    ]);
  });

  it('leaves a sound that plays from its own track in the clip as it is', async () => {
    expect(actions.clipMixOps('c1', 'r1', 'mic', { volume: 0.5 })).toEqual([]);
    actions.setClipSourceMuted('c1', 'r1', 'mic', false);
    await new Promise((r) => setTimeout(r, 20));
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('says why the clip has no mute or volume of its own for a separated sound', () => {
    expect(actions.ownTrackReason('Mic')).toBe('Mic is on its own track here.');
    expect(actions.clipMuteName('Mic', false)).toBe('Mute Mic in this clip');
    expect(actions.clipMuteName('Mic', true)).toBe('Unmute Mic in this clip');
  });
});

describe('a kind of sound in every recording', () => {
  it("a lane's mute is one step named after the lane", async () => {
    actions.setKindMuted('systemAudio', true, 0, 'Computer sound');
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(applied()[0]).toMatchObject({
      label: 'undo.source.mute',
      params: { source: 'Computer sound' },
      ops: [{ type: 'updateRecordingSource', recordingId: 'r1', sourceId: 'computer' }],
    });
  });

  it('a mute that changes nothing sends nothing', async () => {
    actions.setKindMuted('systemAudio', false);
    await new Promise((r) => setTimeout(r, 20));
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it("a lane's volume is built from the project as it goes in", () => {
    // c2's own 60 % goes, and r2's mic, already at 100 %, is said as it is.
    expect(actions.kindVolumeOpsNow('mic', 1, 0).map((op) => op.type)).toEqual([
      'setClipSourceMix',
      'updateRecordingSource',
    ]);
    expect(actions.kindVolumeOpsNow('systemAudio', 1, 0)).toEqual([]);
  });
});

describe('separating and syncing a sound', () => {
  it("Separate makes the clip's part of the sound its own layer, in one step", async () => {
    actions.separateSource('c3', 'r2', 'mic');
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(applied()).toEqual([
      {
        label: 'undo.source.separate',
        params: { source: { code: 'source.role.mic', params: {} } },
        ops: [{ type: 'detachSource', clipId: 'c3', sourceId: 'mic' }],
      },
    ]);
  });

  it('moves a sound 10 ms earlier against the picture, within 5 s either way', async () => {
    actions.nudgeSource('r1', 'mic', -10_000);
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    await settle();
    expect(applied()[0]).toMatchObject({
      label: 'undo.source.sync',
      ops: [{ type: 'updateRecordingSource', recordingId: 'r1', sourceId: 'mic', patch: { nudgeUs: -10_000 } }],
    });
    const snap = twoTakesSnapshot();
    snap.project.recordings[0].sources[1].nudgeUs = actions.MAX_NUDGE_US;
    useProject.setState({ snapshot: snap });
    actions.nudgeSource('r1', 'mic', 10_000);
    await new Promise((r) => setTimeout(r, 20));
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });
});
