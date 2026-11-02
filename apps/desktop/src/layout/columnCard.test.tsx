import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The inspector card in the left column: while something is selected the column shows its card over the library,
 * which stays mounted under it with its view, tab, search and scroll as they were. Back to library in the card's head
 * shows the library again and keeps the selection; picking anything brings the card back. Close and Escape deselect.
 * A folded library opens for the card and folds again after. The preview has no card.
 */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/library', () => ({
  library: {
    pack: vi.fn(() => Promise.resolve([])),
    marks: vi.fn(() => Promise.resolve({})),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../bridge/engine', () => ({
  engine: {
    preview: vi.fn(() => Promise.resolve()),
    previewClear: vi.fn(() => Promise.resolve()),
    seek: vi.fn(() => Promise.resolve()),
    setView: vi.fn(() => Promise.resolve()),
    setRect: vi.fn(() => Promise.resolve()),
  },
}));
vi.mock('../bridge/framing', () => ({
  clipFraming: vi.fn(() => new Promise(() => undefined)),
}));
vi.mock('../bridge/appWindow', () => ({
  appWindow: { fullScreen: vi.fn(() => Promise.resolve(false)), isFullScreen: vi.fn(() => Promise.resolve(false)) },
}));
const { useCaptionsView } = await import('../state/captionsView');
const { useFullScreen } = await import('../state/fullScreen');
const { useLibraryPanel } = await import('../state/libraryPanel');
const { useProject } = await import('../state/project');
const { useSelection } = await import('../state/selection');
const { useUi } = await import('../state/ui');
const { Library } = await import('./Library');
const { Preview } = await import('../preview/Preview');

const panel = () => document.querySelector<HTMLElement>('.library')!;
const body = () => document.querySelector<HTMLElement>('.library-body')!;
const rail = () => document.querySelector<HTMLElement>('.library-rail')!;
/** The card in the left column. */
const card = () => within(panel()).queryByRole('complementary', { name: /settings$/ });
const select = (id: string | null) => act(() => useSelection.getState().select(id));

function Editor() {
  return (
    <>
      <Library />
      <Preview />
    </>
  );
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null, selectedIds: [] });
  useLibraryPanel.setState({ collapsed: false, aside: null });
  useCaptionsView.setState({ view: 'library', open: false });
  useUi.setState({ libraryTab: 'clips', previewFormat: null });
  useFullScreen.setState({ on: false });
});

describe('the card in the left column', () => {
  it('a selection shows its card over the library, and the preview has none', () => {
    renderWithProviders(<Editor />);
    expect(card()).toBeNull();
    expect(panel()).not.toHaveAttribute('data-card');
    select('b');
    expect(card()).toHaveAccessibleName('broll.mp4 settings');
    expect(panel()).toHaveAttribute('data-card');
    // The library stays under it, out of reach of the keyboard and the pointer, but laid out (not `hidden`).
    expect(body()).toHaveAttribute('inert');
    expect(body()).not.toHaveAttribute('hidden');
    expect(within(screen.getByRole('main', { name: 'Preview' })).queryByRole('complementary')).toBeNull();
    // Every kind of card goes there: a title.
    select('title');
    expect(card()).toHaveAccessibleName('Title settings');
    select(null);
    expect(card()).toBeNull();
    expect(panel()).not.toHaveAttribute('data-card');
    expect(body()).not.toHaveAttribute('inert');
  });

  it('the library comes back as it was: its tab, its search and its scroll', async () => {
    renderWithProviders(<Library />);
    await userEvent.click(screen.getByRole('tab', { name: 'Music' }));
    await userEvent.type(screen.getByLabelText('Search music'), 'mus');
    const list = document.querySelector<HTMLElement>('.lib-scroll')!;
    list.scrollTop = 120;
    select('a');
    expect(card()).toHaveAccessibleName('Recording 1 settings');
    select(null);
    expect(card()).toBeNull();
    // The same list, never drawn again from scratch, so its scroll is where it was.
    expect(document.querySelector('.lib-scroll')).toBe(list);
    expect(list.scrollTop).toBe(120);
    expect(screen.getByRole('tab', { name: 'Music' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByLabelText('Search music')).toHaveValue('mus');
  });

  it('full screen shows no card', () => {
    useFullScreen.setState({ on: true });
    renderWithProviders(<Library />);
    select('b');
    expect(card()).toBeNull();
    expect(panel()).not.toHaveAttribute('data-card');
  });
});

describe('back to the library', () => {
  it('Back to library shows the library and keeps the selection; picking the clip again brings the card back', async () => {
    renderWithProviders(<Library />);
    select('b');
    const back = within(card()!).getByRole('button', { name: 'Back to library' });
    expect(back).toHaveAttribute('tabindex', '0');
    act(() => back.focus());
    await userEvent.keyboard('{Enter}');
    expect(card()).toBeNull();
    expect(useSelection.getState().selectedId).toBe('b');
    expect(body()).not.toHaveAttribute('inert');
    expect(screen.getByRole('tab', { name: 'Clips' })).toBeInTheDocument();
    // The keyboard waits on the panel (Space plays from there), never on a button Space would press.
    expect(panel()).toHaveFocus();
    select('b');
    expect(card()).toHaveAccessibleName('broll.mp4 settings');
    // Another pick brings its own card.
    await userEvent.click(within(card()!).getByRole('button', { name: 'Back to library' }));
    expect(card()).toBeNull();
    select('title');
    expect(card()).toHaveAccessibleName('Title settings');
  });

  it('every card has Back to library first in its head', () => {
    renderWithProviders(<Library />);
    for (const [id, name] of [
      ['b', 'broll.mp4 settings'],
      ['title', 'Title settings'],
    ] as const) {
      select(id);
      const head = card()!.querySelector('.insp-head')!;
      expect(card()).toHaveAccessibleName(name);
      expect(head.firstElementChild).toHaveAccessibleName('Back to library');
    }
  });

  it('Escape and Close deselect, as before, and the keyboard stays on the panel', async () => {
    renderWithProviders(<Library />);
    select('b');
    act(() => within(card()!).getByRole('button', { name: 'Close' }).focus());
    await userEvent.keyboard('{Escape}');
    expect(useSelection.getState().selectedId).toBeNull();
    expect(card()).toBeNull();
    expect(panel()).toHaveFocus();
    select('title');
    await userEvent.click(within(card()!).getByRole('button', { name: 'Close' }));
    expect(useSelection.getState().selectedId).toBeNull();
    expect(card()).toBeNull();
  });

  it('the clip card’s Look opens the Looks view in the column, the clip still selected', async () => {
    useLibraryPanel.setState({ collapsed: true });
    renderWithProviders(<Library />);
    select('b');
    await userEvent.click(within(card()!).getByRole('button', { name: /^Look: / }));
    expect(card()).toBeNull();
    expect(useCaptionsView.getState().view).toBe('looks');
    expect(panel()).toHaveAttribute('data-view', 'looks');
    // A folded library opens for it.
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    expect(useSelection.getState().selectedId).toBe('b');
  });
});

describe('a folded library', () => {
  it('opens for the card and folds again when the selection clears', () => {
    useLibraryPanel.setState({ collapsed: true });
    renderWithProviders(<Library />);
    expect(panel()).toHaveAttribute('data-collapsed', 'true');
    expect(rail()).not.toHaveAttribute('hidden');
    select('b');
    expect(card()).toHaveAccessibleName('broll.mp4 settings');
    expect(panel()).toHaveAttribute('data-card');
    expect(rail()).toHaveAttribute('hidden');
    expect(rail()).toHaveAttribute('inert');
    // The library's own setting is untouched.
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    select(null);
    expect(card()).toBeNull();
    expect(rail()).not.toHaveAttribute('hidden');
    expect(screen.getByRole('button', { name: 'Open library' })).toBeInTheDocument();
  });

  it('Back to library goes back to the rail, the selection kept', async () => {
    useLibraryPanel.setState({ collapsed: true });
    renderWithProviders(<Library />);
    select('b');
    await userEvent.click(within(card()!).getByRole('button', { name: 'Back to library' }));
    expect(card()).toBeNull();
    expect(rail()).not.toHaveAttribute('hidden');
    expect(useSelection.getState().selectedId).toBe('b');
  });

  it('Command B with the card in the column folds the column and the card steps aside', () => {
    renderWithProviders(<Library />);
    select('b');
    act(() => useLibraryPanel.getState().toggle());
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(card()).toBeNull();
    expect(rail()).not.toHaveAttribute('hidden');
    expect(useSelection.getState().selectedId).toBe('b');
    // Again: the library opens, the card still aside until the next pick.
    act(() => useLibraryPanel.getState().toggle());
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    expect(card()).toBeNull();
    expect(body()).not.toHaveAttribute('inert');
    // Folded with the card shown there: the column folds back to the rail.
    act(() => useLibraryPanel.getState().setCollapsed(true));
    select('b');
    expect(card()).toBeInTheDocument();
    act(() => useLibraryPanel.getState().toggle());
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(card()).toBeNull();
    expect(rail()).not.toHaveAttribute('hidden');
  });

  it('the column takes the rail’s width only while no card has it', () => {
    const css = readFileSync(resolve(__dirname, '../styles/app.css'), 'utf8');
    expect(css).toContain(
      '.workspace:has(.library.panel[data-collapsed="true"]:not([data-card])) {\n  --library-w: var(--library-rail);\n}',
    );
    // Under the card the library is laid out but not drawn, so its scroll is kept.
    expect(css).toContain('.library[data-card] > .library-body {\n  visibility: hidden;\n}');
  });
});
