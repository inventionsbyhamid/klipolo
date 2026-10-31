import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The preview's header: every control in it does something where it shows. The safe area guide is drawn
 * only in 9:16, so elsewhere its switch is off and says where it shows; full screen isn't there until it works.
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

  it('has no full screen button until full screen works', () => {
    renderWithProviders(<Preview />);
    expect(screen.queryByRole('button', { name: 'Full screen preview' })).toBeNull();
  });
});
