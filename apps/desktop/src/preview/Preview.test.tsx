import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The preview's header: every control in it does something where it shows. Full screen isn't there
 * until it works.
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
  it('has no full screen button until full screen works', () => {
    renderWithProviders(<Preview />);
    expect(screen.queryByRole('button', { name: 'Full screen preview' })).toBeNull();
  });
});
