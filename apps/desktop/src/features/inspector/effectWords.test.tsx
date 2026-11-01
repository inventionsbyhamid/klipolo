import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ContribDef } from '../../bindings/ContribDef';
import { BUILTINS } from '../../lib/contrib';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/* A setting's words in the Effects popover: Escape there puts the words back before it closes the popover. */

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { engineApply, withLayers } = await import('../../test/inspector');
const { Inspector } = await import('./Inspector');

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});

describe('words in the Effects popover', () => {
  it('Escape in a setting’s words puts them back and keeps the stack open; the next closes it', async () => {
    const caption: ContribDef = {
      id: 'com.example/caption',
      kind: 'effect',
      name: 'Caption',
      params: [{ key: 'words', label: 'Words', type: 'text', default: 'Hi', maxLen: 20 }],
    };
    BUILTINS.push(caption);
    try {
      const cap = { id: 'c', effect: caption.id, enabled: true, params: {} };
      withLayers({ id: 'logo-layer', content: { type: 'image', assetId: 'logo' }, effects: [cap] });
      useSelection.setState({ selectedId: 'logo-layer' });
      renderWithProviders(<Inspector format="landscape" />);
      await userEvent.click(screen.getByRole('button', { name: 'Effects 1' }));
      await userEvent.click(screen.getByRole('button', { name: 'Show Caption settings' }));
      const words = screen.getByRole('textbox', { name: 'Words' });
      await userEvent.type(words, ' there');
      await userEvent.keyboard('{Escape}');
      expect(screen.getByRole('dialog', { name: 'Effects' })).toBeInTheDocument();
      expect(words).toHaveValue('Hi');
      await userEvent.keyboard('{Escape}');
      await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Effects' })).toBeNull());
    } finally {
      BUILTINS.splice(BUILTINS.indexOf(caption), 1);
    }
  });
});
