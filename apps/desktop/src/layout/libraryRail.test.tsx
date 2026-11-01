import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/* The library folds to its rail, as chat folds to its strip, and opens again from it. */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/library', () => ({
  library: {
    pack: vi.fn(() => Promise.resolve([])),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
    marks: vi.fn(() => Promise.resolve({})),
  },
}));
const { useProject } = await import('../state/project');
const { useLibraryPanel } = await import('../state/libraryPanel');
const { Library } = await import('./Library');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useLibraryPanel.setState({ collapsed: false });
});

describe('the library folded', () => {
  it('is a rail with Open library; opening it shows the library again', async () => {
    renderWithProviders(<Library />);
    const panel = screen.getByRole('complementary', { name: 'Library' });
    act(() => useLibraryPanel.getState().setCollapsed(true));
    expect(panel).toHaveAttribute('data-collapsed', 'true');
    expect(screen.queryByRole('tab', { name: 'Clips' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Open library' }));
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    expect(panel).toHaveAttribute('data-collapsed', 'false');
    expect(screen.getByRole('tab', { name: 'Clips' })).toBeInTheDocument();
    // The keyboard goes on to the view's tab.
    expect(screen.getByRole('tab', { name: 'Library' })).toHaveFocus();
  });

  it('folded with the keyboard inside, it keeps the keyboard on its rail', () => {
    renderWithProviders(<Library />);
    act(() => screen.getByRole('tab', { name: 'Clips' }).focus());
    act(() => useLibraryPanel.getState().setCollapsed(true));
    expect(screen.getByRole('button', { name: 'Open library' })).toHaveFocus();
  });

  it('Collapse library in its head folds it, as chat has Collapse chat, and the rail keeps the keyboard', async () => {
    renderWithProviders(<Library />);
    const collapse = screen.getByRole('button', { name: 'Collapse library' });
    expect(collapse).toHaveAttribute('title', 'Collapse library (Command B)');
    expect(screen.queryByRole('button', { name: 'Open library' })).toBeNull();
    await userEvent.click(collapse);
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(screen.getByRole('button', { name: 'Open library' })).toHaveFocus();
  });

  it("a double click on its edge folds it, and on the rail's edge opens it again", () => {
    const { container } = renderWithProviders(<Library />);
    const edge = () => container.querySelector<HTMLElement>('[data-library-edge]')!;
    fireEvent.doubleClick(edge());
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    fireEvent.doubleClick(edge());
    expect(useLibraryPanel.getState().collapsed).toBe(false);
  });
});
