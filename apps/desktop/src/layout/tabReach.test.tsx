import { act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The editor from the keyboard, as the Mac's web view walks it. WebKit leaves a plain button out of Tab order while
 * Full Keyboard Access is off (the Mac's default) and keeps it in once it has an explicit tabindex. So every control
 * carries one: 0, or -1 in a roving group whose active item is 0. Tab then reaches every region of the window.
 */

vi.mock('../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../bridge/projects', () => ({
  projects: { recent: vi.fn(() => Promise.resolve([])), reveal: vi.fn() },
}));
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
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
    setView: vi.fn(() => Promise.resolve()),
  },
}));
vi.mock('../bridge/framing', () => ({ clipFraming: vi.fn(() => new Promise(() => undefined)) }));
vi.mock('../bridge/appWindow', () => ({
  appWindow: { fullScreen: vi.fn(() => Promise.resolve(false)), isFullScreen: vi.fn(() => Promise.resolve(false)) },
}));
const { useAi } = await import('../ai/aiStatus');
const { installChatBridge } = await import('../bridge/chat');
const { fakeChat, reply } = await import('../bridge/chatFake');
const { useChat } = await import('../state/chat');
const { useChatPanel } = await import('../state/chatPanel');
const { useCaptionsView } = await import('../state/captionsView');
const { useProject } = await import('../state/project');
const { useSelection } = await import('../state/selection');
const { useUi } = await import('../state/ui');
const { TopBar } = await import('./TopBar');
const { Library } = await import('./Library');
const { Chat } = await import('./Chat');
const { Preview } = await import('../preview/Preview');
const { Timeline } = await import('../features/timeline/Timeline');
const { PlayerBar } = await import('../player/PlayerBar');

/** The window's regions, the more specific first (the inspector and the header are in the preview). */
const REGIONS = [
  ['top bar', '.topbar'],
  ['library', '.library'],
  ['preview header', '.stage-head'],
  ['inspector', '.inspector'],
  // The frame under the header, and what is drawn on it.
  ['preview', '.stage'],
  ['timeline header', '.tl-head'],
  ['timeline', '.timeline'],
  ['chat', '.chat'],
  ['player bar', '.playerbar'],
] as const;

/** What a person presses or picks: a button, or anything that takes a button's, tab's or menu item's role. */
const CONTROLS = [
  'button',
  'a:not([href])',
  '[role="button"]',
  '[role="tab"]',
  '[role="menuitem"]',
  '[role="menuitemradio"]',
  '[role="menuitemcheckbox"]',
  '[role="radio"]',
  '[role="checkbox"]',
  '[role="switch"]',
  '[role="slider"]',
].join(', ');

const regionOf = (el: Element) => REGIONS.find(([, selector]) => el.closest(selector))?.[0] ?? 'elsewhere';
/** Shown and live: not under `hidden` or `inert` (a folded panel's body). */
const live = (el: Element) => !el.closest('[hidden], [inert]');

function Editor() {
  return (
    <>
      <TopBar />
      <div className="workspace">
        <Library />
        <Preview />
        <Timeline />
        <Chat />
      </div>
      <PlayerBar />
    </>
  );
}

beforeEach(() => {
  mockIPC(() => null);
  installChatBridge(fakeChat({ reply: (r) => reply(r.id, 'Done.') }));
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useAi.setState({ status: { mode: 'key' } });
  useChat.getState().reset(fixtureSnapshot().project.id);
  useChatPanel.setState({ collapsed: false });
  useUi.setState({ libraryTab: 'clips' });
  useCaptionsView.setState({ view: 'library' });
  // A clip selected, so its card docks beside the frame.
  useSelection.setState({ selectedId: 'a' });
});

describe('the editor from the keyboard', () => {
  it('every control has a tabindex WebKit honours, and Tab walks every region in order', async () => {
    renderWithProviders(<Editor />);
    await act(() => new Promise((r) => setTimeout(r, 40)));
    const controls = [...document.querySelectorAll<HTMLElement>(CONTROLS)].filter(live);
    // A plain button with no tabindex is one WebKit skips.
    const unmarked = controls.filter((c) => !c.hasAttribute('tabindex'));
    expect(unmarked.map((c) => `${regionOf(c)}: ${c.getAttribute('aria-label') ?? c.textContent}`)).toEqual([]);

    // The walk: from the top of the page until Tab comes round again.
    const stops: HTMLElement[] = [];
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    for (let i = 0; i < 400; i++) {
      await userEvent.tab();
      const at = document.activeElement as HTMLElement;
      if (at === document.body || stops.includes(at)) break;
      stops.push(at);
    }
    // Every stop is one WebKit keeps (a control's tabindex is 0; a text field is always kept).
    for (const stop of stops.filter((s) => s.matches(CONTROLS))) expect(stop).toHaveAttribute('tabindex', '0');
    // Every control marked 0 and not off is reached.
    const tabbable = controls.filter((c) => c.getAttribute('tabindex') === '0' && !c.matches(':disabled'));
    expect(tabbable.filter((c) => !stops.includes(c)).map((c) => c.outerHTML.slice(0, 80))).toEqual([]);
    // The regions, in the window's order.
    const order = stops.map(regionOf).filter((r, i, all) => r !== all[i - 1]);
    expect(order).toEqual([
      'top bar',
      'library',
      'preview header',
      'preview',
      'inspector',
      'timeline header',
      'timeline',
      'chat',
      'player bar',
    ]);
    // The library's view tabs, each its own stop.
    for (const name of ['Library', 'Captions', 'Looks']) {
      const tab = [...document.querySelectorAll<HTMLElement>('.view-tab')].find((el) => el.textContent === name)!;
      expect(stops).toContain(tab);
    }
  });
});
