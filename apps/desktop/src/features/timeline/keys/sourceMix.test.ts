import { describe, expect, it } from 'vitest';
import type { Project } from '../../../bindings/Project';
import type { RecordingSource } from '../../../bindings/RecordingSource';
import { separated, twoTakes } from '../../../test/sounds';
import {
  changesNothing,
  fillGapsOps,
  kindCentreOps,
  kindMuteOps,
  kindVolumeOps,
  liveChannel,
  mutedMix,
  onOwnTrack,
  silent,
  sourceOfKind,
} from './sourceMix';

const mic = (p: Project, r: number) => p.recordings[r].sources.find((s) => s.role === 'mic')!;

describe('a kind of recorded sound in every recording', () => {
  it("a lane's mute sets each take's default and each clip's own setting, never the separated part", () => {
    const p = twoTakes();
    expect(kindMuteOps(p, 'mic', true)).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'mic',
        patch: { mix: { volume: 1, muted: true, effects: [], mutedByPerson: true } },
      },
      // c2's own 60 % stays, muted; c1's mic plays from its own layer, so its mix there is left as it is.
      {
        type: 'setClipSourceMix',
        clipId: 'c2',
        sourceId: 'mic',
        mix: { volume: 0.6, muted: true, effects: [], mutedByPerson: true },
      },
      {
        type: 'updateRecordingSource',
        recordingId: 'r2',
        sourceId: 'mic',
        patch: { mix: { volume: 1, muted: true, effects: [], mutedByPerson: true } },
      },
    ]);
  });

  it("a lane's volume sets every take's, and a clip's own setting that ends up the default is dropped", () => {
    const p = twoTakes();
    expect(kindVolumeOps(p, 'mic', 0.6)).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'mic',
        patch: { mix: { volume: 0.6, muted: false, effects: [] } },
      },
      { type: 'setClipSourceMix', clipId: 'c2', sourceId: 'mic', mix: null },
      {
        type: 'updateRecordingSource',
        recordingId: 'r2',
        sourceId: 'mic',
        patch: { mix: { volume: 0.6, muted: false, effects: [] } },
      },
    ]);
  });

  it('a volume goes in to two decimals', () => {
    const ops = kindVolumeOps(twoTakes(), 'systemAudio', 1.23456);
    expect(ops).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'computer',
        patch: { mix: { volume: 1.23, muted: false, effects: [] } },
      },
    ]);
  });

  it('a take without that kind of sound is left alone', () => {
    const ops = kindMuteOps(twoTakes(), 'systemAudio', true);
    expect(ops.map((op) => ('recordingId' in op ? op.recordingId : op.type))).toEqual(['r1']);
  });

  it("a lane past the first acts on each take's second source of the kind alone", () => {
    const p = twoTakes();
    const lav: RecordingSource = { ...mic(p, 0), id: 'mic-2', label: 'capture.source.mic' };
    p.recordings[0].sources.push(lav);
    expect(sourceOfKind(p.recordings[0], 'mic', 1)?.id).toBe('mic-2');
    expect(sourceOfKind(p.recordings[1], 'mic', 1)).toBeUndefined();
    expect(kindMuteOps(p, 'mic', true, 1)).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'mic-2',
        patch: { mix: { volume: 1, muted: true, effects: [], mutedByPerson: true } },
      },
    ]);
  });

  it("a camera counts only where its files carry sound: a take's camera has none", () => {
    const p = twoTakes();
    p.assets = [...p.assets, { ...p.assets[0], id: 'cam', name: 'cam.mp4', hasAudio: false }];
    const run = { ...mic(p, 0).runs[0], assetId: 'cam' };
    p.recordings[0].sources.push({ ...mic(p, 0), id: 'camera', role: 'camera', runs: [run] });
    expect(kindMuteOps(p, 'camera', true)).toEqual([]);
    // A new asset list, as an edit makes one.
    p.assets = p.assets.map((a) => (a.id === 'cam' ? { ...a, hasAudio: true } : a));
    expect(kindMuteOps(p, 'camera', true)).toHaveLength(1);
  });

  it('changes nothing when every default already is so and no clip changes', () => {
    const p = twoTakes();
    expect(changesNothing(p, kindMuteOps(p, 'systemAudio', false))).toBe(true);
    expect(changesNothing(p, kindMuteOps(p, 'systemAudio', true))).toBe(false);
  });
});

describe('a recorded sound on a layer of its own', () => {
  it("is on its own track where one of its files' layers plays over the clip", () => {
    const p = twoTakes();
    const m = mic(p, 0);
    expect(onOwnTrack(p, 0, m)).toBe(true);
    expect(onOwnTrack(p, 1, m)).toBe(false);
    // A layer of another file never counts.
    p.timeline.tracks[0].items = [separated('other', 's1', 20, 30)];
    expect(onOwnTrack(p, 1, m)).toBe(false);
  });
});

describe('mute and fill', () => {
  it("a mute is the person's and silences the fill; an unmute clears the mark and keeps the fill", () => {
    const filling = { volume: 1, muted: true, effects: [], fillsGaps: true };
    expect(silent(filling)).toBe(false);
    const muted = mutedMix(filling, true);
    expect(muted).toEqual({ volume: 1, muted: true, effects: [], mutedByPerson: true });
    expect(silent(muted)).toBe(true);
    expect(mutedMix({ ...filling, mutedByPerson: true }, false)).toEqual({
      volume: 1,
      muted: false,
      effects: [],
      fillsGaps: true,
    });
  });

  it("filling the gaps turns on each sounding picture source, leaving the person's mute silent", () => {
    const p = twoTakes();
    p.assets = [...p.assets, { ...p.assets[0], id: 'cam', name: 'cam.mp4', hasAudio: true }];
    const camera: RecordingSource = {
      ...mic(p, 0),
      id: 'camera',
      role: 'camera',
      runs: [{ ...mic(p, 0).runs[0], assetId: 'cam' }],
      mix: { volume: 1, muted: true, effects: [] },
    };
    p.recordings[0].sources.push(camera);
    p.timeline.main[1].sourceMix = {
      ...p.timeline.main[1].sourceMix,
      camera: { volume: 1, muted: true, effects: [], mutedByPerson: true },
    };
    expect(fillGapsOps(p, p.recordings[0], true)).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'camera',
        patch: { mix: { volume: 1, muted: true, effects: [], fillsGaps: true } },
      },
    ]);
  });
});

describe('centring a one-sided sound', () => {
  it('plays from the louder side of its two-channel files, else the side it plays from now', () => {
    const p = twoTakes();
    const m = mic(p, 0);
    expect(liveChannel(m, new Map([['m1', [0.001, 0.6]]]))).toBe(1);
    expect(liveChannel(m, new Map([['m1', [0.5, 0.2]]]))).toBe(0);
    expect(liveChannel({ ...m, mix: { ...m.mix, centreFrom: 1 } }, new Map())).toBe(1);
    expect(kindCentreOps(p, 'mic', true, new Map([['m1', [0, 0.6]]]))[0]).toEqual({
      type: 'updateRecordingSource',
      recordingId: 'r1',
      sourceId: 'mic',
      patch: { mix: { volume: 1, muted: false, effects: [], centreFrom: 1 } },
    });
  });
});
