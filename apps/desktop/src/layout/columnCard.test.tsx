import { act, fireEvent, screen, within } from '@testing-library/react';
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
const { usePreview } = await import('../features/media/preview');
const { useEditorShortcuts } = await import('../features/command-palette/useEditorShortcuts');
const { useCaptionsView } = await import('../state/captionsView');
const { usePlayback } = await import('../state/playback');
const { useFullScreen } = await import('../state/fullScreen');
const { useLibraryPanel } = await import('../state/libraryPanel');
const { useProject } = await import('../state/project');
const { useSelection } = await import('../state/selection');
const { useUi } = await import('../state/ui');
const { Library } = await import('./Library');
const { cardWanted } = await import('./columnCard');
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

/** The library with the editor's keys (Space plays the timeline when no control has the keyboard). */
function WithKeys() {
  useEditorShortcuts();
  return <Library />;
}

/** Space where the keyboard is: whether the timeline's play was asked. */
function spacePlays(): boolean {
  const toggle = vi.fn();
  usePlayback.setState({ toggle });
  fireEvent.keyDown(document.activeElement ?? document.body, { key: ' ' });
  return toggle.mock.calls.length === 1;
}

const commandB = () => act(() => useLibraryPanel.getState().toggle(cardWanted()));

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
    // A press on the timeline picks the clip there.
    fireEvent.pointerDown(document.body);
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
    renderWithProviders(<WithKeys />);
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
    expect(spacePlays()).toBe(true);
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

  it('Escape and Close deselect, as before, and the keyboard stays where Space plays', async () => {
    renderWithProviders(<WithKeys />);
    select('b');
    // From a control with no tooltip (an open tooltip takes the first Escape for itself).
    act(() => within(card()!).getByRole('button', { name: 'More' }).focus());
    await userEvent.keyboard('{Escape}');
    expect(useSelection.getState().selectedId).toBeNull();
    expect(card()).toBeNull();
    expect(panel()).toHaveFocus();
    expect(spacePlays()).toBe(true);
    select('title');
    act(() => within(card()!).getByRole('button', { name: 'Close' }).focus());
    await userEvent.keyboard('{Enter}');
    expect(useSelection.getState().selectedId).toBeNull();
    expect(card()).toBeNull();
    expect(panel()).toHaveFocus();
    expect(spacePlays()).toBe(true);
  });

  it('a sound heard from a row stops as the card covers the library', () => {
    renderWithProviders(<Library />);
    act(() => usePreview.setState({ key: 'pack:rain', sounding: true }));
    select('b');
    expect(card()).toBeInTheDocument();
    expect(usePreview.getState().key).toBeNull();
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
    // Back to settings, over the looks, brings the clip's card back.
    await userEvent.click(screen.getByRole('button', { name: 'Back to settings' }));
    expect(card()).toHaveAccessibleName('broll.mp4 settings');
  });

  it('Back to settings shows over the Looks only while the selection’s card is aside', () => {
    useCaptionsView.setState({ view: 'looks', open: false });
    renderWithProviders(<Library />);
    expect(screen.queryByRole('button', { name: 'Back to settings' })).toBeNull();
    select('b');
    act(() => useLibraryPanel.getState().stepAside());
    expect(screen.getByRole('button', { name: 'Back to settings' })).toBeInTheDocument();
    select(null);
    expect(screen.queryByRole('button', { name: 'Back to settings' })).toBeNull();
  });
});

describe('a pick from the library', () => {
  it('keeps the library: a file added from it is selected with its card aside, and a pick elsewhere shows its card', () => {
    renderWithProviders(<Library />);
    const row = within(screen.getByRole('list', { name: 'Your clips' })).getAllByRole('listitem')[0];
    fireEvent.pointerDown(row);
    // The add lands and selects its new clip.
    select('b');
    expect(useSelection.getState().selectedId).toBe('b');
    expect(card()).toBeNull();
    expect(body()).not.toHaveAttribute('inert');
    // A press on the timeline, then a pick there: its card.
    fireEvent.pointerDown(document.body);
    select('a');
    expect(card()).toHaveAccessibleName('Recording 1 settings');
  });

  it('an add from a row’s More menu (in a portal, outside the panel) keeps the library too', async () => {
    renderWithProviders(<Library />);
    await userEvent.click(screen.getByRole('button', { name: 'More for broll.mp4' }));
    const item = screen.getByRole('menuitem', { name: 'Add to the end' });
    expect(panel().contains(item)).toBe(false);
    // A press on its item, in the portal, keeps what the trigger's press said.
    fireEvent.pointerDown(item);
    // The add lands and selects its new clip.
    select('b');
    expect(card()).toBeNull();
    expect(useSelection.getState().selectedId).toBe('b');
  });

  it('a key in the library counts too; a press in the card or a key elsewhere doesn’t', () => {
    renderWithProviders(<Library />);
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Clips' }), { key: 'Enter' });
    select('b');
    expect(card()).toBeNull();
    fireEvent.keyDown(document.body, { key: 'ArrowRight' });
    select('b');
    expect(card()).toHaveAccessibleName('broll.mp4 settings');
    fireEvent.pointerDown(card()!.querySelector('h3')!);
    select('title');
    expect(card()).toHaveAccessibleName('Title settings');
  });
});

describe('a folded library', () => {
  it('keeps its rail column: the card lies over the preview’s edge until the selection clears', () => {
    useLibraryPanel.setState({ collapsed: true });
    renderWithProviders(<Library />);
    expect(panel()).toHaveAttribute('data-collapsed', 'true');
    expect(rail()).not.toHaveAttribute('hidden');
    select('b');
    expect(card()).toHaveAccessibleName('broll.mp4 settings');
    expect(panel()).toHaveAttribute('data-card');
    expect(rail()).toHaveAttribute('hidden');
    expect(rail()).toHaveAttribute('inert');
    // The library stays folded: its column keeps the rail's width (the card is let out over the preview).
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(panel()).toHaveAttribute('data-collapsed', 'true');
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

  it('Command B with the card showing folds the library and the card steps aside', () => {
    renderWithProviders(<Library />);
    select('b');
    commandB();
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(card()).toBeNull();
    expect(rail()).not.toHaveAttribute('hidden');
    expect(useSelection.getState().selectedId).toBe('b');
    // Again: the library opens, the card still aside until the next pick.
    commandB();
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    expect(card()).toBeNull();
    expect(body()).not.toHaveAttribute('inert');
    // Folded with the card shown over the preview: it goes, and the rail stays.
    act(() => useLibraryPanel.getState().setCollapsed(true));
    select('b');
    expect(card()).toBeInTheDocument();
    commandB();
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(card()).toBeNull();
    expect(rail()).not.toHaveAttribute('hidden');
  });

  it('no style sizes the grid’s columns by whether a card shows', () => {
    const css = ['../styles/app.css', '../features/captions/captions.css']
      .map((file) => readFileSync(resolve(__dirname, file), 'utf8'))
      .join('\n');
    const rules = [...css.matchAll(/([^{}]*)\{([^{}]*)\}/g)];
    const byCard = rules.filter(([, selector, body]) => selector.includes('data-card') && /grid-template|--library-w\s*:/.test(body));
    expect(byCard.map(([, selector]) => selector.trim())).toEqual([]);
  });

  it('under the card the library stays laid out, so its scroll is kept, but isn’t drawn', () => {
    renderWithProviders(<Library />);
    select('b');
    expect(body()).toHaveStyle({ visibility: 'hidden' });
    expect(body()).not.toHaveAttribute('hidden');
    select(null);
    expect(body()).not.toHaveStyle({ visibility: 'hidden' });
  });
});
