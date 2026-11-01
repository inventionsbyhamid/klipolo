import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The preview's header: every control in it does something where it shows. The safe area guide is drawn
 * only in 9:16: there its menu picks Off, every platform's area or one app's; elsewhere the button is off and says
 * where it shows. Full screen needs a video to show.
 */

vi.mock('../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../bridge/engine', () => ({
  engine: { seek: vi.fn(() => Promise.resolve()), setView: vi.fn(() => Promise.resolve()) },
}));
vi.mock('../bridge/appWindow', () => ({
  appWindow: { fullScreen: vi.fn(() => Promise.resolve(false)), isFullScreen: vi.fn(() => Promise.resolve(true)) },
}));
const { appWindow } = await import('../bridge/appWindow');
const { useProject } = await import('../state/project');
const { useUi } = await import('../state/ui');
const { useFullScreen } = await import('../state/fullScreen');
const { Preview } = await import('./Preview');

beforeEach(() => {
  useFullScreen.setState({ on: false });
  // An empty project in 16:9 and 9:16: the drop zone shows, under the header.
  const snapshot = fixtureSnapshot();
  snapshot.project.settings.formats = ['landscape', 'portrait'];
  snapshot.project.timeline.main = [];
  useProject.setState({ snapshot, pending: 0 });
  useUi.setState({ previewFormat: null, safeArea: 'off', lastSafeArea: 'all' });
});

describe('the preview’s header', () => {
  it('the safe area is off outside 9:16, saying where it shows; in 9:16 its menu picks the platform', async () => {
    renderWithProviders(<Preview />);
    const off = screen.getByRole('button', { name: 'Safe area' });
    expect(off).toHaveAttribute('aria-disabled', 'true');
    expect(off).toHaveAttribute('title', 'Shows in 9:16');
    await userEvent.click(off);
    expect(screen.queryByRole('menu')).toBeNull();
    expect(useUi.getState().safeArea).toBe('off');
    act(() => useUi.setState({ previewFormat: 'portrait' }));
    const menu = screen.getByRole('button', { name: 'Safe area: Off' });
    expect(menu).toHaveAttribute('title', "Safe area: Off. Show where each app's buttons cover the video.");
    await userEvent.click(menu);
    expect((await screen.findAllByRole('menuitemradio')).map((item) => item.textContent)).toEqual([
      'Off',
      'All platforms',
      'TikTok',
      'Instagram Reels',
      'YouTube Shorts',
    ]);
    expect(screen.getByRole('menuitemradio', { name: 'Off' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'TikTok' }));
    expect(useUi.getState().safeArea).toBe('tiktok');
    expect(screen.getByRole('button', { name: 'Safe area: TikTok' })).toHaveAttribute('data-on');
  });

  it('the chosen area reads off outside 9:16 and is on again back in 9:16', () => {
    useUi.setState({ previewFormat: 'portrait', safeArea: 'reels' });
    renderWithProviders(<Preview />);
    expect(screen.getByRole('button', { name: 'Safe area: Instagram Reels' })).toHaveAttribute('data-on');
    act(() => useUi.setState({ previewFormat: 'landscape' }));
    expect(screen.getByRole('button', { name: 'Safe area' })).not.toHaveAttribute('data-on');
    // Back in 9:16, the guide is on again.
    act(() => useUi.setState({ previewFormat: 'portrait' }));
    expect(screen.getByRole('button', { name: 'Safe area: Instagram Reels' })).toHaveAttribute('data-on');
  });

  it('the safe area button keeps the keyboard focus as the format changes', () => {
    useUi.setState({ previewFormat: 'portrait' });
    renderWithProviders(<Preview />);
    const button = screen.getByRole('button', { name: 'Safe area: Off' });
    act(() => button.focus());
    act(() => useUi.setState({ previewFormat: 'landscape' }));
    expect(screen.getByRole('button', { name: 'Safe area' })).toBe(button);
    expect(button).toHaveFocus();
    expect(button).toHaveAttribute('aria-disabled', 'true');
    act(() => useUi.setState({ previewFormat: 'portrait' }));
    expect(button).toHaveFocus();
    expect(button).not.toHaveAttribute('aria-disabled');
  });

  it('Off from the menu hides the guide, and the next drag shows the area chosen last', async () => {
    useUi.setState({ previewFormat: 'portrait', safeArea: 'shorts', lastSafeArea: 'shorts' });
    renderWithProviders(<Preview />);
    await userEvent.click(screen.getByRole('button', { name: 'Safe area: YouTube Shorts' }));
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Off' }));
    expect(useUi.getState()).toMatchObject({ safeArea: 'off', lastSafeArea: 'shorts' });
    expect(screen.getByRole('button', { name: 'Safe area: Off' })).not.toHaveAttribute('data-on');
  });

  it('Full screen is off with no video yet, and says why', async () => {
    renderWithProviders(<Preview />);
    const full = screen.getByRole('button', { name: 'Full screen preview' });
    expect(full).toHaveAttribute('aria-disabled', 'true');
    expect(full).toHaveAttribute('title', 'Add a video to watch it full screen');
    await userEvent.click(full);
    expect(useFullScreen.getState().on).toBe(false);
  });

  it('Full screen fills the screen with the preview, and the same button leaves', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    renderWithProviders(<Preview />);
    const full = screen.getByRole('button', { name: 'Full screen preview' });
    expect(full).toHaveAttribute('title', 'Full screen preview (F)');
    await userEvent.click(full);
    expect(useFullScreen.getState().on).toBe(true);
    expect(appWindow.fullScreen).toHaveBeenLastCalledWith(true);
    const leave = screen.getByRole('button', { name: 'Leave full screen' });
    expect(leave).toHaveAttribute('title', 'Leave full screen (Esc)');
    await userEvent.click(leave);
    expect(useFullScreen.getState().on).toBe(false);
    expect(appWindow.fullScreen).toHaveBeenLastCalledWith(false);
  });
});
