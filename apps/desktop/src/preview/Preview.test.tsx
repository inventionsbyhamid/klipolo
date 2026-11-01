import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The preview's header: every control in it does something where it shows. The safe area guide is drawn
 * only in 9:16, so elsewhere its switch is off and says where it shows; full screen needs a video to show.
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
  it('Show safe area is off outside 9:16, saying where it shows; in 9:16 it shows the guide', async () => {
    renderWithProviders(<Preview />);
    const safe = screen.getByRole('button', { name: 'Show safe area' });
    expect(safe).toHaveAttribute('aria-disabled', 'true');
    expect(safe).toHaveAttribute('title', 'Shows in 9:16');
    await userEvent.click(safe);
    expect(useUi.getState().safeArea).toBe('off');
    act(() => useUi.setState({ previewFormat: 'portrait' }));
    expect(safe).not.toHaveAttribute('aria-disabled');
    await userEvent.click(safe);
    expect(useUi.getState().safeArea).toBe('all');
    expect(safe).toHaveAttribute('aria-pressed', 'true');
  });

  it('Show safe area reads off outside 9:16, with the guide on for 9:16', () => {
    useUi.setState({ previewFormat: 'portrait', safeArea: 'all' });
    renderWithProviders(<Preview />);
    const safe = screen.getByRole('button', { name: 'Show safe area' });
    expect(safe).toHaveAttribute('aria-pressed', 'true');
    act(() => useUi.setState({ previewFormat: 'landscape' }));
    expect(safe).toHaveAttribute('aria-pressed', 'false');
    // Back in 9:16, the guide is on again.
    act(() => useUi.setState({ previewFormat: 'portrait' }));
    expect(safe).toHaveAttribute('aria-pressed', 'true');
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
