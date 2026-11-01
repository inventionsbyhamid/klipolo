import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The preview's header: every control in it does something where it shows. The safe area guide is drawn
 * only in 9:16: there its menu picks Off, every platform's area or one app's; elsewhere the button is off and says
 * where it shows. Full screen isn't there until it works.
 */

vi.mock('../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../bridge/engine', () => ({ engine: {} }));
const { useProject } = await import('../state/project');
const { useUi } = await import('../state/ui');
const { Preview } = await import('./Preview');

beforeEach(() => {
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

  it('Off from the menu hides the guide, and the next drag shows the area chosen last', async () => {
    useUi.setState({ previewFormat: 'portrait', safeArea: 'shorts', lastSafeArea: 'shorts' });
    renderWithProviders(<Preview />);
    await userEvent.click(screen.getByRole('button', { name: 'Safe area: YouTube Shorts' }));
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Off' }));
    expect(useUi.getState()).toMatchObject({ safeArea: 'off', lastSafeArea: 'shorts' });
    expect(screen.getByRole('button', { name: 'Safe area: Off' })).not.toHaveAttribute('data-on');
  });

  it('has no full screen button until full screen works', () => {
    renderWithProviders(<Preview />);
    expect(screen.queryByRole('button', { name: 'Full screen preview' })).toBeNull();
  });
});
