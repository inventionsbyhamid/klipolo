import { afterEach, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('./project');
const { usePlayback } = await import('./playback');
const { keepPlayheadInVideo, keepPlayheadOnReload, restorePlayhead } = await import('./transport');

let stop = () => undefined as void;
afterEach(() => stop());

it('an undo that makes the video shorter brings the playhead back inside it', () => {
  const snapshot = fixtureSnapshot();
  useProject.setState({ snapshot, pending: 0 });
  usePlayback.setState({ playheadUs: 28_000_000, playing: false });
  stop = keepPlayheadInVideo();
  // broll.mp4 (20 to 30 s) goes: the video is 20 s long, and the playhead was at 28 s.
  const shorter = structuredClone(snapshot);
  shorter.project.timeline.main = shorter.project.timeline.main.slice(0, 1);
  shorter.project.timeline.tracks = shorter.project.timeline.tracks.map((t) => ({ ...t, items: [] }));
  useProject.setState({ snapshot: shorter });
  expect(usePlayback.getState().playheadUs).toBe(20_000_000);
  // A longer video leaves it where it is.
  useProject.setState({ snapshot });
  expect(usePlayback.getState().playheadUs).toBe(20_000_000);
});

it('the playhead keeps its place across a reload of the page, in the same project only', () => {
  const snapshot = fixtureSnapshot();
  useProject.setState({ snapshot, pending: 0 });
  usePlayback.setState({ playheadUs: 12_500_000, playing: false });
  stop = keepPlayheadOnReload();
  window.dispatchEvent(new Event('pagehide'));
  // The page comes back: the store starts over at 0, the project opens again.
  usePlayback.setState({ playheadUs: 0 });
  restorePlayhead();
  expect(usePlayback.getState().playheadUs).toBe(12_500_000);
  // Taken once: a later reload with nothing noted leaves it where it is.
  usePlayback.setState({ playheadUs: 0 });
  restorePlayhead();
  expect(usePlayback.getState().playheadUs).toBe(0);
  // Another project opened meanwhile: not its place.
  window.dispatchEvent(new Event('pagehide'));
  useProject.setState({ snapshot: { ...snapshot, project: { ...snapshot.project, id: 'other' } } });
  restorePlayhead();
  expect(usePlayback.getState().playheadUs).toBe(0);
});
