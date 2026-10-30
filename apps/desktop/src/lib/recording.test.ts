import { describe, expect, it } from 'vitest';
import type { Asset } from '../bindings/Asset';
import type { Recording } from '../bindings/Recording';
import type { RecordingSource } from '../bindings/RecordingSource';
import type { SourceRole } from '../bindings/SourceRole';
import type { SourceRun } from '../bindings/SourceRun';
import { sourceAtUs } from '../features/timeline/media/runs';
import { cameraAssets, layerRate, runAt, runInClip, screenSource, soundSources } from './recording';
import parity from './recording-parity.json';

const S = 1_000_000;
const run = (assetId: string, from: number, len: number): SourceRun => ({
  assetId,
  offsetUs: from * S,
  durationUs: len * S,
  driftPpm: 0,
});
const source = (id: string, role: SourceRole, runs: SourceRun[]): RecordingSource => ({
  id,
  role,
  label: `capture.source.${id}`,
  runs,
  nudgeUs: 0,
  mix: { volume: 1, muted: false, effects: [] },
});
const recording = (primary: string, sources: RecordingSource[]): Recording => ({
  id: 'rec',
  kind: primary === 'camera' ? 'camera' : 'screen',
  sources,
  primary,
  markers: [],
});

describe('recording sources', () => {
  it('has no screen when the camera is the main footage', () => {
    const r = recording('camera', [source('camera', 'camera', [run('c', 0, 60)])]);
    expect(screenSource(r)).toBeUndefined();
    expect(cameraAssets(r)).toEqual([]);
  });

  it('lists every file of a camera that reconnected', () => {
    const camera = source('camera', 'camera', [run('c1', 0, 10), run('c2', 12, 48)]);
    const r = recording('screen', [source('screen', 'screen', [run('s', 0, 60)]), camera]);
    expect(screenSource(r)?.id).toBe('screen');
    expect(cameraAssets(r)).toEqual(['c1', 'c2']);
  });

  it('plays nothing in a gap between runs, and each run from its own start', () => {
    const mic = source('mic', 'mic', [run('m1', 0, 10), run('m2', 12, 48)]);
    expect(runAt(mic, 11 * S)).toBeNull();
    expect(runAt(mic, 15 * S)).toEqual({ run: mic.runs[1], localUs: 3 * S });
    expect(runAt({ ...mic, nudgeUs: -S }, 11 * S)?.run.assetId).toBe('m2');
  });

  it('maps runs onto the timeline exactly as the model does, drift and nudge included', () => {
    const { sources, clips, spans, at } = parity as unknown as {
      sources: RecordingSource[];
      clips: { sourceInUs: number; sourceOutUs: number; speed: number; startUs: number }[];
      spans: { clip: number; source: string; run: number; startUs: number; endUs: number; runInUs: number }[];
      at: { source: string; us: number; assetId: string | null; localUs: number | null }[];
    };
    expect(sources.some((s) => s.nudgeUs !== 0 && s.runs.some((r) => r.driftPpm !== 0))).toBe(true);
    const got = clips.flatMap((clip, index) =>
      sources.flatMap((s) =>
        s.runs.flatMap((run, r) => {
          const hit = runInClip(clip, clip.startUs, s, run);
          return hit ? [{ clip: index, source: s.id, run: r, ...hit }] : [];
        }),
      ),
    );
    expect(got).toEqual(spans);
    for (const point of at) {
      const hit = runAt(
        sources.find((s) => s.id === point.source)!,
        point.us,
      );
      expect({ ...point, assetId: hit?.run.assetId ?? null, localUs: hit?.localUs ?? null }).toEqual(point);
    }
  });

  it("draws a separated sound layer's file at its run's rate, where the engine plays it", () => {
    const { layers } = parity as unknown as {
      layers: { driftPpm: number; sourceInUs: number; offsetUs: number; fileUs: number }[];
    };
    expect(layers.some((l) => l.driftPpm !== 0 && l.fileUs !== l.sourceInUs + l.offsetUs)).toBe(true);
    for (const l of layers) {
      // What a paint maps by (`sourceAtUs`) at the speed `ItemBlock` gives a sound layer (`layerRate`): within the
      // half µs Rust rounds to.
      const clock = { sourceInUs: l.sourceInUs, speed: layerRate({ driftPpm: l.driftPpm }) };
      const drawn = sourceAtUs(clock, 0, l.offsetUs);
      expect(Math.abs(drawn - l.fileUs), JSON.stringify(l)).toBeLessThanOrEqual(0.5);
    }
  });

  it('soundSources_matches_rust: a synced camera’s own sound is heard, a take’s silent picture files aren’t', () => {
    const golden = (
      parity as unknown as {
        soundSources: {
          assets: Asset[];
          recordings: Recording[];
          found: { recording: string; sources: string[] }[];
        };
      }
    ).soundSources;
    expect(golden.found.map((f) => f.recording)).toEqual(['interview', 'take']);
    for (const { recording, sources } of golden.found) {
      const r = golden.recordings.find((x) => x.id === recording)!;
      expect(
        soundSources(golden, r).map((s) => s.id),
        recording,
      ).toEqual(sources);
    }
  });
});
