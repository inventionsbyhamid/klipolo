import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
import type { Transaction } from '../../bindings/Transaction';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn(), snapshot: vi.fn() },
}));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn().mockResolvedValue(undefined), previewClear: vi.fn(), seek: vi.fn() },
}));
const { bridge } = await import('../../bridge/commands');
const { useExport } = await import('../../state/export');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { useProject } = await import('../../state/project');
const { FormatPills } = await import('./FormatPills');
const { switchFormat } = await import('./formats');
const { TopBar } = await import('../../layout/TopBar');

/** Rust's side: the transaction applied as one step, named by its label (as `Document::commit_tx` keeps it). */
function landing(tx: Transaction): Snapshot {
  const now = useProject.getState().snapshot!;
  const op = tx.ops[0];
  const settings = op.type === 'setSettings' ? op.settings : now.project.settings;
  return {
    ...now,
    project: { ...now.project, settings },
    history: {
      ...now.history,
      revision: now.history.revision + 1,
      canUndo: true,
      undoLabel: tx.label,
      undoParams: tx.params,
    },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  vi.mocked(bridge.apply).mockImplementation((tx) =>
    Promise.resolve({ ...fixturePatch(landing(tx)), baseRevision: useProject.getState().snapshot!.history.revision }),
  );
});

describe('format switch', () => {
  it('a pill switches the format as one setSettings op, and the top bar names it for Undo', async () => {
    renderWithProviders(
      <>
        <TopBar />
        <FormatPills />
      </>,
    );
    const pills = screen.getByRole('radiogroup', { name: 'Format' });
    expect(screen.getByRole('radio', { name: '16:9' })).toBeChecked();
    // Every format is offered: one the project lacks joins it.
    expect(screen.getAllByRole('radio').map((r) => r.textContent)).toEqual(['16:9', '9:16', '1:1']);
    await userEvent.click(screen.getByRole('radio', { name: '9:16' }));
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'format.switch.undo',
      params: { ratio: '9:16' },
      ops: [{ type: 'setSettings', settings: { fps: 30, formats: ['landscape', 'portrait'], primaryFormat: 'portrait' } }],
    });
    await vi.waitFor(() => expect(screen.getByRole('radio', { name: '9:16' })).toBeChecked());
    expect(pills).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Undo switch to 9:16' })).toBeEnabled();
  });

  it('the format already shown changes nothing', async () => {
    renderWithProviders(<FormatPills />);
    await userEvent.click(screen.getByRole('radio', { name: '16:9' }));
    expect(await switchFormat('landscape')).toBe(true);
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('two quick switches each build on the one before', async () => {
    await Promise.all([switchFormat('portrait'), switchFormat('square')]);
    const sent = vi.mocked(bridge.apply).mock.calls.map(([tx]) => tx.ops[0]);
    expect(sent).toEqual([
      { type: 'setSettings', settings: { fps: 30, formats: ['landscape', 'portrait'], primaryFormat: 'portrait' } },
      {
        type: 'setSettings',
        settings: { fps: 30, formats: ['landscape', 'portrait', 'square'], primaryFormat: 'square' },
      },
    ]);
  });

  it('2 switches to 9:16 from the keyboard, and Command E opens the export dialog', async () => {
    function Keys() {
      useEditorShortcuts();
      return null;
    }
    renderWithProviders(<Keys />);
    fireEvent.keyDown(document.body, { key: '2' });
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(vi.mocked(bridge.apply).mock.calls[0][0].label).toBe('format.switch.undo');
    fireEvent.keyDown(document.body, { key: 'e', metaKey: true });
    expect(useExport.getState().open).toBe(true);
  });
});
