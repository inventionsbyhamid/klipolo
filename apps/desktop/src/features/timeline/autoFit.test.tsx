import { act } from '@testing-library/react';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The timeline shows the whole video when a project opens, and when an empty one gets its first clips, so what was
 * just added isn't off to the right. Once the person zooms, their zoom stays.
 */

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { zoomTimeline } = await import('./zoomControl');

// 1200 px of lanes after the 40 px gutter; a fit leaves a 24 px margin.
const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

const S = 1_000_000;
/** The zoom that shows `seconds` of video in the lanes. */
const fit = (seconds: number) => 1176 / seconds;

/** A new project, `id`, with no clips (the fixture's files and layers taken out), and a 3 minute video to add. */
function empty(id = 'new'): Snapshot {
  const snapshot = fixtureSnapshot();
  const { project } = snapshot;
  project.id = id;
  project.assets = [{ ...project.assets[5], id: 'long', path: '/media/long.mp4', name: 'long.mp4', durationUs: 180 * S }];
  project.recordings = [];
  project.timeline.main = [];
  for (const track of project.timeline.tracks) track.items = [];
  return snapshot;
}

/** `snapshot` with `long.mp4` (or `seconds` of it) added at the end of the main track, one revision on. */
function withClip(snapshot: Snapshot, id: string, seconds = 180): Snapshot {
  const clip: Clip = {
    ...fixtureSnapshot().project.timeline.main[1],
    id,
    source: { type: 'asset', id: 'long' },
    sourceInUs: 0,
    sourceOutUs: seconds * S,
  };
  const { project, history } = snapshot;
  return {
    ...snapshot,
    project: { ...project, timeline: { ...project.timeline, main: [...project.timeline.main, clip] } },
    history: { ...history, revision: history.revision + 1 },
  };
}

const pps = () => useUi.getState().pxPerSecond;

beforeEach(() => {
  useProject.setState({ snapshot: null, pending: 0 });
  useUi.setState({ pxPerSecond: 40 });
});

describe('the timeline’s zoom as clips come in', () => {
  it('an empty project fits the video once its first clips come in, and not again', async () => {
    await act(() => useProject.getState().load(empty()));
    renderWithProviders(<Timeline />);
    // Nothing to fit yet.
    expect(pps()).toBe(40);
    act(() => useProject.setState({ snapshot: withClip(useProject.getState().snapshot!, 'c1') }));
    expect(pps()).toBeCloseTo(fit(180), 6);
    // Another clip later leaves the zoom where it is.
    act(() => useProject.setState({ snapshot: withClip(useProject.getState().snapshot!, 'c2') }));
    expect(pps()).toBeCloseTo(fit(180), 6);
  });

  it('a project that opens shows its whole video, and so does the next one opened', async () => {
    await act(() => useProject.getState().load(withClip(empty('a'), 'c1')));
    renderWithProviders(<Timeline />);
    expect(pps()).toBeCloseTo(fit(180), 6);
    act(() => useUi.getState().setPxPerSecond(20));
    await act(() => useProject.getState().load(withClip(empty('b'), 'c1', 60)));
    expect(pps()).toBeCloseTo(fit(60), 6);
  });

  it('never refits after the person zooms', async () => {
    await act(() => useProject.getState().load(empty()));
    renderWithProviders(<Timeline />);
    act(() => zoomTimeline(2));
    expect(pps()).toBe(80);
    act(() => useProject.setState({ snapshot: withClip(useProject.getState().snapshot!, 'c1') }));
    expect(pps()).toBe(80);
  });
});
