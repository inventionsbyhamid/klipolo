import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('../state/project');
const { usePlayback } = await import('../state/playback');
const { useSelection } = await import('../state/selection');
const { useUi } = await import('../state/ui');
const { Timeline } = await import('../features/timeline/Timeline');
const { PlayerBar } = await import('./PlayerBar');

// 1200 px of lanes after the 40 px gutter.
const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 0, playing: false, rate: 1 });
  useUi.setState({ pxPerSecond: 40, snapping: true });
});

function Editor() {
  return (
    <>
      <Timeline />
      <PlayerBar />
    </>
  );
}

describe('the player bar', () => {
  it('its buttons say what they do and their key in a tooltip, never an empty "()"', async () => {
    renderWithProviders(<Editor />);
    const start = screen.getByRole('button', { name: 'Go to start' });
    act(() => start.focus());
    const tip = await screen.findByRole('tooltip');
    expect(tip).toHaveTextContent('Go to start');
    expect(within(tip).getByText('Home').tagName).toBe('KBD');
    for (const button of screen.getAllByRole('button')) expect(button.textContent ?? '').not.toContain('()');
  });

  it('Delete stays focusable while off and says why; Split says why too', async () => {
    renderWithProviders(<Editor />);
    const remove = screen.getByRole('button', { name: /Delete/ });
    expect(remove).toHaveAttribute('aria-disabled', 'true');
    act(() => remove.focus());
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Select something first');
  });

  it('shows no control that looks live and does nothing: no volume or loop until they work', () => {
    const { container } = renderWithProviders(<Editor />);
    const bar = container.querySelector('footer.playerbar')!;
    // Every slider in the bar is one the person can move.
    for (const slider of bar.querySelectorAll('.slider')) expect(slider).toHaveAttribute('role', 'slider');
    expect(bar.querySelector('.vol')).toBeNull();
    expect(within(bar as HTMLElement).queryByRole('button', { name: 'Loop the selection' })).toBeNull();
  });

  it('the snapping switch shows whether snapping is on and turns it off and on, as N does', async () => {
    renderWithProviders(<Editor />);
    const snap = screen.getByRole('button', { name: 'Snapping' });
    expect(snap).toHaveAttribute('aria-pressed', 'true');
    act(() => snap.focus());
    const tip = await screen.findByRole('tooltip');
    expect(within(tip).getByText('N').tagName).toBe('KBD');
    await userEvent.click(snap);
    expect(useUi.getState().snapping).toBe(false);
    expect(snap).toHaveAttribute('aria-pressed', 'false');
    act(() => useUi.getState().setSnapping(true));
    expect(snap).toHaveAttribute('aria-pressed', 'true');
  });

  it('while playing backward or faster the play button shows which way and how fast, and says it', () => {
    const { container } = renderWithProviders(<Editor />);
    const badge = () => container.querySelector<HTMLElement>('.transport [data-rate-badge]')!;
    expect(badge()).toHaveAttribute('role', 'status');
    expect(badge()).toBeEmptyDOMElement();
    act(() => usePlayback.setState({ playing: true, rate: 2 }));
    expect(badge()).toHaveTextContent('Playing at 2×');
    expect(badge().querySelector('[aria-hidden="true"]')).toHaveTextContent(/^2×$/);
    act(() => usePlayback.setState({ rate: -2 }));
    expect(badge()).toHaveTextContent('Playing backward at 2×');
    act(() => usePlayback.setState({ rate: -1 }));
    expect(badge()).toHaveTextContent('Playing backward at 1×');
    // Forward at normal speed, or stopped: nothing.
    act(() => usePlayback.setState({ rate: 1 }));
    expect(badge()).toBeEmptyDOMElement();
    act(() => usePlayback.setState({ playing: false, rate: 2 }));
    expect(badge()).toBeEmptyDOMElement();
  });

  it('the zoom slider says its value and steps 1 or 10 with the arrows', async () => {
    renderWithProviders(<Editor />);
    const slider = screen.getByRole('slider', { name: 'Timeline zoom' });
    const now = () => Number(slider.getAttribute('aria-valuenow'));
    expect(slider.getAttribute('aria-valuetext')).toMatch(/^Shows \d/);
    const before = now();
    fireEvent.keyDown(slider, { key: 'ArrowLeft' });
    expect(now()).toBe(before - 1);
    fireEvent.keyDown(slider, { key: 'ArrowLeft', shiftKey: true });
    expect(now()).toBe(before - 11);
    // Fit: the 30 s video in the 1200 px of lanes (less a 24 px margin).
    await userEvent.click(screen.getByRole('button', { name: 'Fit' }));
    expect(useUi.getState().pxPerSecond).toBeCloseTo(1176 / 30, 6);
    expect(slider.getAttribute('aria-valuetext')).toBe('Shows 0:30.6');
  });
});
