import { render, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixturePatch, fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn(), snapshot: vi.fn() },
}));
const { bridge } = await import('../bridge/commands');
const { useProject } = await import('../state/project');
const { useSaving } = await import('../state/saving');
const { EXPORT_LATER, TopBar } = await import('./TopBar');

beforeEach(() => {
  useProject.setState({ snapshot: null, pending: 0 });
  useSaving.setState({ failing: null, saved: null });
});

describe('TopBar', () => {
  it('says why Export is off, with and without a video', () => {
    const { rerender } = render(<TopBar />);
    const exportButton = () => screen.getByRole('button', { name: 'Export' });
    expect(exportButton()).toBeDisabled();
    expect(exportButton().title).toBe('Add a video first');
    useProject.setState({ snapshot: fixtureSnapshot() });
    rerender(<TopBar />);
    expect(exportButton().title).toBe(EXPORT_LATER);
  });

  it('names the change Undo takes back, and undoes it', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(bridge.undo).mockResolvedValue({ ...fixturePatch(fixtureSnapshot()), baseRevision: 3 });
    render(<TopBar />);
    expect(screen.getByRole('button', { name: 'Redo' })).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Undo add text' }));
    expect(bridge.undo).toHaveBeenCalledTimes(1);
  });

  it('says the project is saved, saving, or why it is not', () => {
    const snapshot = { ...fixtureSnapshot(), opening: 1 };
    useProject.setState({ snapshot });
    useSaving.setState({ saved: { opening: 1, revision: snapshot.history.revision } });
    const { rerender } = render(<TopBar />);
    expect(screen.getByText('Saved')).toBeInTheDocument();
    useProject.setState({ snapshot: { ...snapshot, history: { ...snapshot.history, revision: 4 } } });
    rerender(<TopBar />);
    expect(screen.getByText('Saving…')).toBeInTheDocument();
    useSaving.setState({ failing: 'Your disk is full, so changes aren’t being saved. Free up some space.' });
    rerender(<TopBar />);
    expect(screen.getByText('Not saved').title).toMatch(/disk is full/);
  });
});
