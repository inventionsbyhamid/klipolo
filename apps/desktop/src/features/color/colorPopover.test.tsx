import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

// The seam is mocked: no test ever shows the system's eyedropper.
vi.mock('../../bridge/screenColor', () => ({ screenColor: { supported: vi.fn(() => true), pick: vi.fn() } }));
const { screenColor } = await import('../../bridge/screenColor');
const { useProject } = await import('../../state/project');
const { useRecentColors } = await import('../../state/recentColors');
const { flushBursts } = await import('../../state/bursts');
const { ColorField } = await import('./ColorField');
type Props = Parameters<typeof ColorField>[0];

const PRESETS = [
  { value: '#FFFFFF', name: 'White' },
  { value: '#121A33', name: 'Navy' },
  { value: '#FFD02E', name: 'Yellow' },
  { value: '#7F92DA', name: 'Blue' },
  { value: '#000000', name: 'Black' },
];
function handlers() {
  return { onPreview: vi.fn(), onPreviewEnd: vi.fn(), onCommit: vi.fn() };
}
const field = (h: ReturnType<typeof handlers>, props: Partial<Props> = {}) =>
  renderWithProviders(<ColorField label="Text color" value="#FFFFFF" presets={PRESETS} {...h} {...props} />);
const open = (name = 'Text color: more colors') => userEvent.click(screen.getByRole('button', { name }));
const dialog = () => screen.getByRole('dialog', { name: 'Text color' });
const names = (group: string) =>
  within(within(dialog()).getByRole('group', { name: group }))
    .getAllByRole('option')
    .map((o) => o.getAttribute('aria-label'));
/** The colour area, 100 x 100 px from the page's top left. */
function area() {
  const el = screen.getByTestId('color-area');
  el.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 100 }) as DOMRect;
  return el;
}

beforeEach(() => {
  useRecentColors.setState({ recent: [] });
  useProject.setState({ snapshot: null });
  vi.mocked(screenColor.supported).mockReturnValue(true);
  vi.mocked(screenColor.pick).mockReset();
});
afterEach(() => vi.useRealTimers());

describe('the color popover', () => {
  it('opens on the selected swatch, dry-running nothing, with Klipolo’s colors, the video’s and Recent', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.captions = { ...snapshot.project.captions, accent: '#32D74B' };
    useProject.setState({ snapshot });
    useRecentColors.setState({ recent: ['#AA1122', '#FFD02E'] });
    const h = handlers();
    field(h, { label: 'Outline', value: '#000000', none: { label: 'No outline' } });
    await open('Outline: more colors');
    const popover = screen.getByRole('dialog', { name: 'Outline' });
    expect(within(popover).getByRole('option', { name: 'Black' })).toHaveFocus();
    expect(h.onPreview).not.toHaveBeenCalled();
    const group = (name: string) =>
      within(within(popover).getByRole('group', { name })).getAllByRole('option').map((o) => o.getAttribute('aria-label'));
    expect(group('Klipolo')).toEqual(['No outline', 'White', 'Navy', 'Yellow', 'Blue', 'Black']);
    expect(group('In this video')).toEqual(['Green']);
    // A preset never repeats under Recent.
    expect(group('Recent')).toEqual(['Red']);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('a custom color is one commit and goes first in Recent; a preset never does', async () => {
    const h = handlers();
    field(h);
    await open();
    const hex = within(dialog()).getByRole('textbox', { name: 'Hex color' });
    await userEvent.clear(hex);
    await userEvent.type(hex, '12AB34{Enter}');
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#12AB34');
    expect(useRecentColors.getState().recent).toEqual(['#12AB34']);
    await userEvent.click(within(dialog()).getByRole('option', { name: 'Yellow' }));
    expect(h.onCommit).toHaveBeenLastCalledWith('#FFD02E');
    expect(useRecentColors.getState().recent).toEqual(['#12AB34']);
    await userEvent.keyboard('{Escape}');
    await open();
    expect(names('Recent')).toEqual(['Green']);
  });

  it('a drag on the color area dry-runs as it moves and commits once, on release', async () => {
    const h = handlers();
    field(h);
    await open();
    const el = area();
    fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: 100, clientY: 0 });
    fireEvent.pointerMove(el, { pointerId: 1, clientX: 100, clientY: 50 });
    expect(h.onPreview).toHaveBeenLastCalledWith('#800000');
    expect(h.onCommit).not.toHaveBeenCalled();
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 100, clientY: 50 });
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#800000');
    expect(useRecentColors.getState().recent).toEqual(['#800000']);
  });

  it('a cancelled drag puts the video back and commits nothing', async () => {
    const h = handlers();
    field(h);
    await open();
    const el = area();
    fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: 100, clientY: 0 });
    fireEvent.pointerCancel(el, { pointerId: 1 });
    expect(h.onPreviewEnd).toHaveBeenCalled();
    expect(h.onCommit).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog', { name: 'Text color' })).toBeInTheDocument();
  });

  it('a run of keys on the area is one commit once they stop; an undo commits it first', async () => {
    const h = handlers();
    field(h, { value: '#FF0000' });
    await open('Text color: Red, more colors');
    const saturation = within(dialog()).getByRole('slider', { name: 'Saturation' });
    vi.useFakeTimers();
    act(() => saturation.focus());
    for (let i = 0; i < 3; i++) fireEvent.keyDown(saturation, { key: 'ArrowLeft' });
    expect(h.onPreview).toHaveBeenCalledTimes(3);
    expect(h.onCommit).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(400));
    expect(h.onCommit).toHaveBeenCalledOnce();
    fireEvent.keyDown(saturation, { key: 'ArrowLeft' });
    act(() => void flushBursts());
    expect(h.onCommit).toHaveBeenCalledTimes(2);
  });

  it('with an alpha, Opacity changes only the alpha', async () => {
    const h = handlers();
    field(h, { label: 'Highlight', value: '#121A33DB', alpha: true, none: { label: 'No highlight' } });
    await open('Highlight: more colors');
    const opacity = within(screen.getByRole('dialog', { name: 'Highlight' })).getByRole('slider', { name: 'Opacity' });
    expect(opacity).toHaveAttribute('aria-valuenow', '86');
    vi.useFakeTimers();
    act(() => opacity.focus());
    fireEvent.keyDown(opacity, { key: 'End' });
    act(() => vi.advanceTimersByTime(400));
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#121A33FF');
  });

  it('the eyedropper’s color is one commit and goes to Recent; a cancel commits nothing; a failure says so', async () => {
    vi.mocked(screenColor.pick).mockResolvedValueOnce('#12AB34').mockResolvedValueOnce(null);
    const h = handlers();
    field(h);
    await open();
    const dropper = within(dialog()).getByRole('button', { name: 'Pick a color from the screen' });
    await userEvent.click(dropper);
    await waitFor(() => expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#12AB34'));
    expect(useRecentColors.getState().recent).toEqual(['#12AB34']);
    await userEvent.click(dropper);
    await act(async () => undefined);
    expect(h.onCommit).toHaveBeenCalledTimes(1);
    vi.mocked(screenColor.pick).mockRejectedValueOnce(new Error('no'));
    await userEvent.click(dropper);
    expect(await within(dialog()).findByText('Couldn’t pick a color from the screen. Try again.')).toBeInTheDocument();
  });

  it('shows no eyedropper where the computer has none', async () => {
    vi.mocked(screenColor.supported).mockReturnValue(false);
    field(handlers());
    await open();
    expect(within(dialog()).queryByRole('button', { name: 'Pick a color from the screen' })).toBeNull();
  });
});
