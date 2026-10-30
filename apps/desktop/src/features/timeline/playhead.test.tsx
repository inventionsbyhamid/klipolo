import { act, fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ pxPerSecond: 40 });
});

describe('the playhead as a slider', () => {
  it('is a labelled slider over the video that says where it is, and follows the playhead', () => {
    renderWithProviders(<Timeline />);
    const slider = screen.getByRole('slider', { name: 'Playhead' });
    expect(slider).toHaveAttribute('aria-valuemin', '0');
    expect(slider).toHaveAttribute('aria-valuemax', '30000000');
    expect(slider).toHaveAttribute('aria-valuenow', '0');
    expect(slider).toHaveAttribute('aria-valuetext', '0 seconds');
    act(() => usePlayback.getState().seek(26_400_000));
    expect(slider).toHaveAttribute('aria-valuenow', '26400000');
    expect(slider).toHaveAttribute('aria-valuetext', '26.4 seconds');
    act(() => usePlayback.getState().seek(0));
  });

  it('moves a frame with the arrows, a second with Shift, and to either end with Home and End', () => {
    renderWithProviders(<Timeline />);
    const slider = screen.getByRole('slider', { name: 'Playhead' });
    act(() => slider.focus());
    fireEvent.keyDown(slider, { key: 'ArrowRight' });
    // 30 fps: one frame.
    expect(usePlayback.getState().playheadUs).toBe(33_333);
    fireEvent.keyDown(slider, { key: 'ArrowRight', shiftKey: true });
    expect(usePlayback.getState().playheadUs).toBe(1_033_333);
    fireEvent.keyDown(slider, { key: 'ArrowLeft' });
    expect(usePlayback.getState().playheadUs).toBe(1_000_000);
    fireEvent.keyDown(slider, { key: 'End' });
    expect(usePlayback.getState().playheadUs).toBe(30_000_000);
    expect(slider).toHaveAttribute('aria-valuetext', '30 seconds');
    fireEvent.keyDown(slider, { key: 'Home' });
    expect(usePlayback.getState().playheadUs).toBe(0);
  });
});
