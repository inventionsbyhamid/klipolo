import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/engine', () => ({
  engine: { seek: vi.fn(() => Promise.resolve()), play: vi.fn(), pause: vi.fn() },
}));
const { engine } = await import('../bridge/engine');
const { useProject } = await import('../state/project');
const { usePlayback } = await import('../state/playback');
const { TimelinePanel, ruler } = await import('./TimelinePanel');

beforeEach(() => {
  window.requestAnimationFrame = (run: FrameRequestCallback) => {
    run(0);
    return 0;
  };
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false, seq: 0 });
  vi.mocked(engine.seek).mockClear();
});

describe('ruler', () => {
  it('labels an empty timeline every 5 s over 40 s', () => {
    const r = ruler(0);
    expect(r.labels.map((l) => l.text)).toEqual(['0:00', '0:05', '0:10', '0:15', '0:20', '0:25', '0:30', '0:35']);
    expect(r.major).toBe('12.5%');
    expect(r.minor).toBe('2.5%');
  });

  it('picks a step that gives at most 8 labels', () => {
    const r = ruler(155_000_000);
    expect(r.labels.map((l) => l.text)).toEqual(['0:00', '0:30', '1:00', '1:30', '2:00', '2:30']);
  });
});

describe('the timeline panel', () => {
  it('draws each clip of the main track in order, as long as it plays', () => {
    render(<TimelinePanel />);
    // The fixture's main track: 20 s of the recording, then the 10 s b-roll (30 s in all).
    const blocks = screen.getAllByTestId('clip-block');
    expect(blocks.map((b) => b.textContent)).toEqual(['screen.mp4', 'broll.mp4']);
    expect(blocks.map((b) => [b.style.left, b.style.width])).toEqual([
      ['0%', `${(20 / 30) * 100}%`],
      [`${(20 / 30) * 100}%`, `${(10 / 30) * 100}%`],
    ]);
  });

  it('seeks to where the timeline is clicked, each time with a newer sequence', () => {
    render(<TimelinePanel />);
    const bar = screen.getByRole('slider', { name: 'Playhead' });
    bar.getBoundingClientRect = () => ({ left: 100, width: 300, top: 0, height: 20 }) as DOMRect;
    fireEvent.pointerDown(bar, { clientX: 150, button: 0, pointerId: 1 });
    fireEvent.pointerUp(bar, { clientX: 150, pointerId: 1 });
    fireEvent.pointerDown(bar, { clientX: 500, button: 0, pointerId: 1 });
    const calls = vi.mocked(engine.seek).mock.calls;
    expect(calls.map(([at]) => at)).toEqual([5_000_000, 30_000_000]);
    expect(calls[1][1]).toBeGreaterThan(calls[0][1]);
    expect(bar.getAttribute('aria-valuetext')).toBe('0:30.0');
  });
});
