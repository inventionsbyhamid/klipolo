import { describe, expect, it } from 'vitest';
import type { Project } from '../bindings/Project';
import { fixtureProject } from '../test/fixtures';
import { analysable, recordingOf, voiceOf } from './analysed';

const ids = (project: Project) => analysable(project).map((a) => a.id);

/** The fixture project with its recording's sources of `roles` only. */
function withSources(roles: string[]): Project {
  const project = fixtureProject();
  const [recording] = project.recordings;
  const sources = recording.sources.filter((s) => roles.includes(s.role));
  return { ...project, recordings: [{ ...recording, sources }] };
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
    noMic.assets = noMic.assets.map((a) => (a.id === 'camera' ? { ...a, hasAudio: true } : a));
    expect(ids(noMic)).toEqual(['camera', 'broll']);
    // No file of the take has sound: it has no voice to offer.
    noMic.assets = noMic.assets.map((a) => (a.kind === 'video' ? { ...a, hasAudio: false } : a));
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
