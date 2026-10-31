import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The note the preview shows when its engine isn't available: what happened, and a way back that doesn't need the
 * project closed and opened again.
 */

vi.mock('../../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    setView: vi.fn(() => Promise.resolve()),
    setRect: vi.fn(() => Promise.resolve()),
  },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { useEngine } = await import('../../state/engine');
const { Stage } = await import('./Stage');

beforeAll(() => {
  // jsdom has no layout: give the stage an 800x450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useEngine.setState({ status: 'unavailable', message: 'The preview has stopped.', attempt: 0 });
});

describe('the preview’s failure note', () => {
  it('says what happened and offers Try again, which starts the preview again', async () => {
    renderWithProviders(<Stage />);
    expect(screen.getByText('The preview has stopped.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(useEngine.getState().attempt).toBe(1);
    expect(useEngine.getState().status).toBe('starting');
    expect(screen.queryByText('The preview has stopped.')).toBeNull();
    // The press is the button's alone: it doesn't select the clip under the note.
    expect(useSelection.getState().selectedId).toBeNull();
  });
});
