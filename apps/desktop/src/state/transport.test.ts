import { afterEach, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('./project');
const { usePlayback } = await import('./playback');
const { keepPlayheadInVideo } = await import('./transport');

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
