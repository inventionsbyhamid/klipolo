import { describe, expect, it } from 'vitest';
import type { Project } from '../bindings/Project';
import { fixtureProject } from '../test/fixtures';
import { analysable, recordingOf, voiceOf } from './analysed';

const S = 1_000_000;

const ids = (project: Project) => analysable(project).map((a) => a.id);

/** The fixture project with its recording's sources of `roles` only. */
function withSources(roles: string[]): Project {
  const project = fixtureProject();
  const [recording] = project.recordings;
  const sources = recording.sources.filter((s) => roles.includes(s.role));
  return { ...project, recordings: [{ ...recording, sources }] };
}

/** `project`'s files of `ids` with sound, its other pictures without. */
function sounding(project: Project, ids: string[]) {
  project.assets = project.assets.map((a) => (a.kind === 'video' ? { ...a, hasAudio: ids.includes(a.id) } : a));
}

describe('what analysis offers', () => {
  it('counts a recording as one clip, by its voice: the mic first', () => {
    // The fixture's take (screen, camera, mic) and the b-roll; its song is a layer.
    expect(ids(fixtureProject())).toEqual(['mic', 'broll']);
    expect(voiceOf(fixtureProject(), 'mic')).toBe('rec');
    expect(voiceOf(fixtureProject(), 'screen')).toBeNull();
    expect(voiceOf(fixtureProject(), 'broll')).toBeNull();
  });

  it('takes a sounding camera, then the screen, when there is no mic', () => {
    const noMic = withSources(['screen', 'camera']);
    // The fixture's camera has no sound: the screen's is the voice.
    expect(ids(noMic)).toEqual(['screen', 'broll']);
    sounding(noMic, ['camera', 'broll']);
    expect(ids(noMic)).toEqual(['camera', 'broll']);
    // No file of the take has sound: it has no voice to offer.
    sounding(noMic, ['broll']);
    expect(ids(noMic)).toEqual(['broll']);
  });

  it('offers a recording’s file a clip plays on its own, and nothing of a take off the main track', () => {
    const project = fixtureProject();
    const [take, broll] = project.timeline.main;
    project.timeline.main = [broll, { ...broll, id: 'c', source: { type: 'asset', id: 'screen' } }];
    expect(ids(project)).toEqual(['screen', 'broll']);
    expect(voiceOf(project, 'screen')).toBeNull();
    project.timeline.main = [take];
    expect(ids(project)).toEqual(['mic']);
  });

  it('offers only the voice files the take’s clips play', () => {
    // The mic reconnected at 30 s into a second file; the take's clip plays its first 20 s.
    const project = fixtureProject();
    project.assets = [...project.assets, { ...project.assets[2], id: 'mic2' }];
    const mic = project.recordings[0].sources[2];
    mic.runs = [
      { ...mic.runs[0], durationUs: 30 * S },
      { assetId: 'mic2', offsetUs: 30 * S, durationUs: 30 * S, driftPpm: 0 },
    ];
    expect(ids(project)).toEqual(['mic', 'broll']);
    project.timeline.main = [{ ...project.timeline.main[0], sourceInUs: 25 * S, sourceOutUs: 40 * S }];
    expect(ids(project)).toEqual(['mic', 'mic2']);
  });

  it('takes a picture source’s own sound where it fills the take’s gaps, as the captions do', () => {
    // A synced camera with its own sound, muted to fill the stretch before the mic started, 5 s in.
    const project = fixtureProject();
    sounding(project, ['camera', 'broll']);
    const [screen, camera, mic] = project.recordings[0].sources;
    camera.mix = { ...camera.mix, muted: true, fillsGaps: true };
    mic.runs = [{ ...mic.runs[0], offsetUs: 5 * S, durationUs: 55 * S }];
    expect(ids(project)).toEqual(['camera', 'mic', 'broll']);
    expect(voiceOf(project, 'camera')).toBe('rec');
    // A clip after the gap has none to fill.
    project.timeline.main = [{ ...project.timeline.main[0], sourceInUs: 5 * S }];
    expect(ids(project)).toEqual(['mic']);
    // Muted with no fill, the camera's sound plays nowhere.
    project.timeline.main = fixtureProject().timeline.main;
    const silent = { ...camera, mix: { ...camera.mix, fillsGaps: false } };
    project.recordings = [{ ...project.recordings[0], sources: [screen, silent, mic] }];
    expect(ids(project)).toEqual(['mic', 'broll']);
  });

  it('gives the same array while the main track, assets and recordings stay the same', () => {
    const project = fixtureProject();
    expect(analysable(project)).toBe(analysable({ ...project }));
    expect(analysable({ ...project, recordings: [...project.recordings] })).not.toBe(analysable(project));
  });

  it('knows every file of a recording, analysed for its words only', () => {
    for (const id of ['screen', 'camera', 'mic']) expect(recordingOf(fixtureProject(), id)).toBe('rec');
    expect(recordingOf(fixtureProject(), 'broll')).toBeNull();
  });
});
