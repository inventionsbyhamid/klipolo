import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderWithProviders } from '../test/render';

vi.mock('../bridge/engine', () => ({ engine: { setVolume: vi.fn(() => Promise.resolve()) } }));
const { engine } = await import('../bridge/engine');
const { followVolume, heardVolume, sendVolume, useVolume } = await import('../state/volume');
const { VolumeControl } = await import('./VolumeControl');

let stop = () => undefined as void;
beforeEach(() => {
  localStorage.clear();
  useVolume.setState({ level: 1, muted: false });
  vi.mocked(engine.setVolume).mockClear();
  stop = followVolume();
});
afterEach(() => stop());

describe("the player bar's volume", () => {
  it('starts at full volume and steps 5 (Shift 20) with the arrows', () => {
    renderWithProviders(<VolumeControl />);
    const slider = screen.getByRole('slider', { name: 'Preview volume' });
    expect(slider).toHaveAttribute('aria-valuenow', '100');
    expect(slider).toHaveAttribute('aria-valuetext', '100%');
    fireEvent.keyDown(slider, { key: 'ArrowLeft' });
    expect(slider).toHaveAttribute('aria-valuenow', '95');
    fireEvent.keyDown(slider, { key: 'ArrowDown', shiftKey: true });
    expect(slider).toHaveAttribute('aria-valuenow', '75');
    fireEvent.keyDown(slider, { key: 'Home' });
    expect(slider).toHaveAttribute('aria-valuenow', '0');
    fireEvent.keyDown(slider, { key: 'ArrowRight' });
    fireEvent.keyDown(slider, { key: 'End' });
    expect(slider).toHaveAttribute('aria-valuenow', '100');
  });

  it('is what the preview plays at, never above full: the slider on a loudness curve, silent while muted', () => {
    useVolume.getState().setLevel(0.5);
    expect(engine.setVolume).toHaveBeenLastCalledWith(0.25);
    useVolume.getState().setLevel(2);
    expect(useVolume.getState().level).toBe(1);
    expect(engine.setVolume).toHaveBeenLastCalledWith(1);
    useVolume.getState().toggleMute();
    expect(engine.setVolume).toHaveBeenLastCalledWith(0);
    expect(heardVolume({ level: 0.8, muted: true })).toBe(0);
    // Sent again as the preview attaches (a new engine starts at full volume).
    vi.mocked(engine.setVolume).mockClear();
    sendVolume();
    expect(engine.setVolume).toHaveBeenCalledWith(0);
  });

  it('Mute silences the preview and Unmute brings back the level it had; moving the slider unmutes', async () => {
    renderWithProviders(<VolumeControl />);
    const slider = screen.getByRole('slider', { name: 'Preview volume' });
    act(() => useVolume.getState().setLevel(0.6));
    await userEvent.click(screen.getByRole('button', { name: 'Mute' }));
    expect(slider).toHaveAttribute('aria-valuetext', 'Muted');
    expect(slider).toHaveAttribute('aria-valuenow', '0');
    await userEvent.click(screen.getByRole('button', { name: 'Unmute' }));
    expect(slider).toHaveAttribute('aria-valuenow', '60');
    expect(engine.setVolume).toHaveBeenLastCalledWith(expect.closeTo(0.36));
    await userEvent.click(screen.getByRole('button', { name: 'Mute' }));
    fireEvent.keyDown(slider, { key: 'ArrowRight' });
    expect(useVolume.getState()).toMatchObject({ muted: false, level: 0.65 });
  });

  it('a press on the slider sets the level where it lands', () => {
    renderWithProviders(<VolumeControl />);
    const slider = screen.getByRole('slider', { name: 'Preview volume' });
    slider.getBoundingClientRect = () => ({ left: 100, width: 80, top: 0, height: 16 }) as DOMRect;
    fireEvent.pointerDown(slider, { clientX: 120, pointerId: 1 });
    expect(useVolume.getState().level).toBe(0.25);
  });

  it('is kept on this Mac for the next time Klipolo opens', () => {
    useVolume.getState().setLevel(0.4);
    useVolume.getState().toggleMute();
    expect(JSON.parse(localStorage.getItem('klipolo.volume') ?? 'null')).toEqual({ level: 0.4, muted: true });
  });
});
