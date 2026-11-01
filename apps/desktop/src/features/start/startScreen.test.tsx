import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectRow } from '../../bindings/ProjectRow';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

// `projects_recent` goes through the real bridge to this `invoke`; creating and opening go through the project bridge.
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('../../bridge/commands', () => ({
  bridge: { createProject: vi.fn(), openProject: vi.fn(), duplicateProject: vi.fn(), snapshot: vi.fn() },
  mediaUrl: (path: string) => `asset://localhost${path}`,
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useToasts } = await import('../../state/toasts');
const { StartScreen } = await import('./StartScreen');

const MINUTE = 60_000;
const NOW = new Date(2026, 9, 31, 12, 0).getTime();

const ROWS: ProjectRow[] = [
  {
    path: '/Users/me/Movies/Klipolo/Studio tour.klipolo',
    name: 'Studio tour',
    openedMs: NOW - 2 * 60 * MINUTE,
    durationUs: 252_000_000,
    clips: 3,
    thumb: { sprite: '/cache/media/thumbs/tour/c-00000.k.jpg', tileW: 128, tileH: 72, cols: 8 },
  },
  {
    path: '/Users/me/Movies/Klipolo/Untitled project.klipolo',
    name: 'Untitled project',
    openedMs: NOW - 5 * MINUTE,
    durationUs: 0,
    clips: 0,
    thumb: null,
  },
  {
    path: '/Users/me/Movies/Klipolo/Kiln vlog.klipolo',
    name: 'Kiln vlog',
    openedMs: new Date(2026, 9, 12, 9, 30).getTime(),
    durationUs: 61_400_000,
    clips: 1,
    // A phone video: a tall tile, drawn whole in the middle of the 16:9 picture.
    thumb: { sprite: '/cache/media/thumbs/kiln/c-00000.k.jpg', tileW: 40, tileH: 72, cols: 8 },
  },
];

/** `projects_recent` answers with `rows`, or fails with `error`; each call takes the next answer. */
function recentAnswers(...answers: (ProjectRow[] | Error)[]) {
  invoke.mockImplementation(async (command: string) => {
    if (command !== 'projects_recent') throw new Error(`unexpected command ${command}`);
    const next = answers.length > 1 ? answers.shift()! : answers[0];
    if (next instanceof Error) throw next;
    return next;
  });
}

// The cards themselves, not their More buttons.
const cards = () =>
  within(screen.getByRole('list'))
    .getAllByRole('button')
    .filter((button) => button.classList.contains('start-card'));
const card = (name: string) => cards().find((c) => c.textContent?.includes(name))!;

beforeEach(() => {
  vi.spyOn(Date, 'now').mockReturnValue(NOW);
  useProject.setState({ snapshot: null, pending: 0, switching: false });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.createProject).mockResolvedValue(fixtureSnapshot());
  vi.mocked(bridge.openProject).mockResolvedValue(fixtureSnapshot());
});

afterEach(() => {
  vi.restoreAllMocks();
  invoke.mockReset();
  vi.mocked(bridge.createProject).mockReset();
  vi.mocked(bridge.openProject).mockReset();
  vi.mocked(bridge.duplicateProject).mockReset();
});

describe('StartScreen', () => {
  it('lists_projects_with_name_opened_time_and_duration', async () => {
    let answer!: (rows: ProjectRow[]) => void;
    invoke.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    renderWithProviders(<StartScreen onRecord={() => undefined} />);
    const main = screen.getByRole('main', { name: 'Your projects' });
    expect(within(main).getByRole('heading', { name: 'Your projects' })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('projects_recent', undefined);
    // For a moment only the title: a list that lands by then shows at once, with nothing on the way.
    expect(within(main).queryByRole('status')).toBeNull();
    expect(within(main).queryAllByRole('button')).toEqual([]);
    // Then loading: a spinner and one line, with the header's two actions.
    expect(await within(main).findByRole('status')).toHaveTextContent('Loading your projects');
    expect(main.querySelector('.spinner')).not.toBeNull();
    expect(within(main).getByRole('button', { name: 'New project' })).toBeInTheDocument();
    expect(within(main).getByRole('button', { name: 'Record your screen' })).toBeInTheDocument();

    answer(ROWS);
    await waitFor(() => expect(cards()).toHaveLength(3));
    expect(screen.queryByText('Loading your projects')).not.toBeInTheDocument();
    // Last opened first, as Rust lists them.
    expect(cards().map((c) => c.querySelector('.start-name')?.textContent)).toEqual([
      'Studio tour',
      'Untitled project',
      'Kiln vlog',
    ]);
    expect(cards().map((c) => c.querySelector('.start-meta')?.textContent)).toEqual([
      'Opened 2 hours ago, 4:12',
      'Opened 5 minutes ago, 0:00',
      'Opened Oct 12, 1:01',
    ]);
    // A long name is cut with an ellipsis; the whole name is in its tooltip.
    expect(card('Studio tour').querySelector('.start-name')).toHaveAttribute('title', 'Studio tour');
    // A card is named by its name and described by its line.
    expect(screen.getByRole('button', { name: 'Studio tour' })).toHaveAccessibleDescription(
      'Opened 2 hours ago, 4:12',
    );
  });

  it('unknown_open_time_shows_only_the_length', async () => {
    recentAnswers([{ ...ROWS[2], openedMs: 0 }]);
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(1));
    expect(card('Kiln vlog').querySelector('.start-meta')).toHaveTextContent(/^1:01$/);
    expect(screen.getByRole('button', { name: 'Kiln vlog' })).toHaveAccessibleDescription('1:01');
  });

  it('card_draws_its_tile_or_the_clip_art', async () => {
    const wide: ProjectRow = {
      ...ROWS[1],
      path: '/Users/me/Movies/Klipolo/Banner.klipolo',
      name: 'Banner',
      // A 4:1 picture: a tile wider than the card's 16:9.
      thumb: { sprite: '/cache/media/thumbs/banner/c-00000.k.jpg', tileW: 288, tileH: 72, cols: 8 },
    };
    recentAnswers([...ROWS, wide]);
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(4));

    // Tile 0 of the sprite: the sheet is 8 tiles wide, so it is drawn 8 times the tile's width, from its top left.
    const tile = card('Studio tour').querySelector<HTMLElement>('.start-tile')!;
    expect(tile.style.backgroundImage).toBe('url("asset://localhost/cache/media/thumbs/tour/c-00000.k.jpg")');
    // (jsdom shortens "800% auto" to its equal, "800%".)
    expect(tile.style.backgroundSize).toMatch(/^800%( auto)?$/);
    expect(tile.style.backgroundPosition).toBe('0px 0px');
    expect([tile.style.width, tile.style.height]).toEqual(['100%', '100%']);
    // The clip art is drawn first, under the tile: a picture that doesn't load leaves it showing.
    const pic = card('Studio tour').querySelector('.start-pic')!;
    expect([...pic.children].map((el) => (el.tagName.toLowerCase() === 'svg' ? 'art' : el.className))).toEqual([
      'art',
      'start-tile',
    ]);

    // A tall tile keeps its shape: the full height, 40/72 of 9/16 of the width.
    const tall = card('Kiln vlog').querySelector<HTMLElement>('.start-tile')!;
    expect(tall.style.backgroundImage).toBe('url("asset://localhost/cache/media/thumbs/kiln/c-00000.k.jpg")');
    expect(tall.style.height).toBe('100%');
    expect(parseFloat(tall.style.width)).toBeCloseTo(31.25, 2);

    // A wide tile is letterboxed: the full width, 16/9 of 72/288 of the height.
    const banner = card('Banner').querySelector<HTMLElement>('.start-tile')!;
    expect(banner.style.width).toBe('100%');
    expect(parseFloat(banner.style.height)).toBeCloseTo(44.44, 2);

    // No thumbnail yet (an empty project, or a clip not scanned): the film clip art.
    const art = card('Untitled project');
    expect(art.querySelector('.start-tile')).toBeNull();
    expect(art.querySelector('.start-pic svg.ico')).not.toBeNull();
  });

  it('opening_a_card_opens_its_project', async () => {
    recentAnswers(ROWS);
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));

    // From the keyboard: Tab goes to New project, then the first card; Enter opens it.
    await userEvent.tab();
    expect(screen.getByRole('button', { name: 'New project' })).toHaveFocus();
    await userEvent.tab();
    expect(card('Studio tour')).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(bridge.openProject).toHaveBeenCalledWith('/Users/me/Movies/Klipolo/Studio tour.klipolo');
    await waitFor(() => expect(useProject.getState().pending).toBe(0));
    expect(useProject.getState().snapshot).not.toBeNull();

    // A click opens one too.
    await userEvent.click(card('Kiln vlog'));
    expect(bridge.openProject).toHaveBeenLastCalledWith('/Users/me/Movies/Klipolo/Kiln vlog.klipolo');
    expect(bridge.openProject).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(useProject.getState().pending).toBe(0));
  });

  it("a card's menu duplicates its project and opens the copy", async () => {
    recentAnswers(ROWS);
    const copy = fixtureSnapshot();
    copy.project = { ...copy.project, id: 'copy', name: 'Studio tour copy' };
    vi.mocked(bridge.duplicateProject).mockResolvedValue({ ...copy, opening: 1 });
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    // From the keyboard: the card's More button is the next stop after it.
    act(() => card('Studio tour').focus());
    await userEvent.tab();
    const more = screen.getByRole('button', { name: 'More for Studio tour' });
    expect(more).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Duplicate' }));
    expect(bridge.duplicateProject).toHaveBeenCalledWith('/Users/me/Movies/Klipolo/Studio tour.klipolo');
    await waitFor(() => expect(useProject.getState().snapshot?.project.name).toBe('Studio tour copy'));
    expect(bridge.openProject).not.toHaveBeenCalled();
  });

  it('a copy that fails says why and lists the projects again', async () => {
    recentAnswers(ROWS);
    vi.mocked(bridge.duplicateProject).mockRejectedValue(
      new Error("Klipolo couldn't create the project. Check the disk, then try again."),
    );
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    await userEvent.click(screen.getByRole('button', { name: 'More for Kiln vlog' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Duplicate' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([
      "Klipolo couldn't create the project. Check the disk, then try again.",
    ]);
    expect(useProject.getState().snapshot).toBeNull();
  });

  it('opens one project at a time', async () => {
    recentAnswers(ROWS);
    // Every open made lands at the end, so the store's queue is free for the next test whatever happens here.
    const opens: (() => void)[] = [];
    vi.mocked(bridge.openProject).mockImplementation(
      () => new Promise((resolve) => opens.push(() => resolve(fixtureSnapshot()))),
    );
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    await userEvent.click(card('Kiln vlog'));
    // The press took: the card is busy while it opens.
    expect(card('Kiln vlog')).toHaveAttribute('aria-busy', 'true');
    expect(card('Kiln vlog')).toHaveClass('is-busy');
    expect(card('Kiln vlog').querySelector('.start-busy .spinner')).not.toBeNull();
    expect(card('Studio tour')).not.toHaveAttribute('aria-busy');
    // A second press while the first is opening does nothing, nor does New project.
    await userEvent.click(card('Studio tour'));
    await userEvent.click(screen.getByRole('button', { name: 'New project' }));
    // The store runs them one after another, so whatever was queued reaches Rust only as each open lands: let each
    // land, then count.
    for (let i = 0; i < 5 && useProject.getState().pending > 0; i++) {
      opens.splice(0).forEach((open) => open());
      await waitFor(() => expect(opens.length > 0 || useProject.getState().pending === 0).toBe(true));
    }
    expect(useProject.getState().pending).toBe(0);
    expect(bridge.openProject).toHaveBeenCalledTimes(1);
    expect(bridge.openProject).toHaveBeenCalledWith('/Users/me/Movies/Klipolo/Kiln vlog.klipolo');
    expect(bridge.createProject).not.toHaveBeenCalled();
    await waitFor(() => expect(card('Kiln vlog')).not.toHaveAttribute('aria-busy'));
  });

  it('a_failed_open_lists_the_projects_again', async () => {
    // The file went (moved or deleted) after the list was read.
    recentAnswers(
      ROWS,
      ROWS.filter((row) => row.name !== 'Kiln vlog'),
    );
    vi.mocked(bridge.openProject).mockRejectedValue(new Error("That project isn't there any more."));
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    act(() => card('Kiln vlog').focus());
    await userEvent.keyboard('{Enter}');
    // The store says why; the list is read again, quietly, and the card that went is gone.
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual(["That project isn't there any more."]);
    expect(screen.queryByText('Loading your projects')).not.toBeInTheDocument();
    // The focus went with the card: it goes on to the first card, not to the page.
    expect(card('Studio tour')).toHaveFocus();
  });

  it('a_card_that_still_cannot_be_opened_keeps_the_focus', async () => {
    recentAnswers(ROWS);
    vi.mocked(bridge.openProject).mockRejectedValue(new Error("Klipolo couldn't read this project."));
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    act(() => card('Kiln vlog').focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(card('Kiln vlog')).not.toHaveAttribute('aria-busy'));
    expect(card('Kiln vlog')).toHaveFocus();
  });

  it('new_project_creates_untitled_with_three_formats', async () => {
    recentAnswers(ROWS);
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    await userEvent.click(screen.getByRole('button', { name: 'New project' }));
    expect(bridge.createProject).toHaveBeenCalledWith('Untitled project', ['landscape', 'portrait', 'square']);
    await waitFor(() => expect(useProject.getState().snapshot).not.toBeNull());
  });

  it('record_opens_the_recorder', async () => {
    recentAnswers(ROWS);
    const onRecord = vi.fn();
    renderWithProviders(<StartScreen onRecord={onRecord} />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    const record = screen.getByRole('button', { name: 'Record your screen' });
    // The record glyph, in the button's own ink.
    expect(record.querySelector('svg.ico')).not.toBeNull();
    await userEvent.click(record);
    expect(onRecord).toHaveBeenCalledTimes(1);
    expect(bridge.createProject).not.toHaveBeenCalled();
  });

  it('record_is_hidden_without_a_recorder', async () => {
    recentAnswers(ROWS);
    renderWithProviders(<StartScreen />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    expect(screen.queryByRole('button', { name: 'Record your screen' })).toBeNull();
    expect(screen.getByRole('button', { name: 'New project' })).toBeInTheDocument();
  });

  it('a_quick_empty_list_shows_its_buttons_only_under_the_invitation', async () => {
    recentAnswers([]);
    renderWithProviders(<StartScreen onRecord={() => undefined} />);
    const head = screen.getByRole('main', { name: 'Your projects' }).querySelector('header')!;
    // Watched from the first render: the buttons never show in the header, so nothing moves when the list is empty.
    const shownInHead: string[] = [];
    const watch = new MutationObserver(() =>
      shownInHead.push(...[...head.querySelectorAll('button')].map((b) => b.textContent ?? '')),
    );
    watch.observe(head, { childList: true, subtree: true });
    expect(head.querySelectorAll('button')).toHaveLength(0);
    await screen.findByRole('heading', { name: 'No projects yet' });
    await new Promise((resolve) => setTimeout(resolve, 300));
    watch.disconnect();
    expect(shownInHead).toEqual([]);
    expect(head.querySelectorAll('button')).toHaveLength(0);
    expect(screen.getAllByRole('button', { name: 'New project' })).toHaveLength(1);
  });

  it('a_slow_empty_list_takes_the_focus_with_the_buttons', async () => {
    let answer!: (rows: ProjectRow[]) => void;
    invoke.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    renderWithProviders(<StartScreen onRecord={() => undefined} />);
    await screen.findByRole('status');
    // The person is on Record your screen in the header while the list is read.
    await userEvent.tab();
    await userEvent.tab();
    expect(screen.getByRole('button', { name: 'Record your screen' })).toHaveFocus();
    answer([]);
    await screen.findByRole('heading', { name: 'No projects yet' });
    // The same button, under the invitation now, keeps the focus.
    const record = screen.getByRole('button', { name: 'Record your screen' });
    expect(record.closest('.start-empty')).not.toBeNull();
    expect(record).toHaveFocus();
  });

  it('an_empty_library_without_a_recorder_invites_a_project', async () => {
    recentAnswers([]);
    renderWithProviders(<StartScreen />);
    const main = screen.getByRole('main', { name: 'Your projects' });
    expect(await within(main).findByRole('heading', { name: 'No projects yet' })).toBeInTheDocument();
    // Until there is a recorder, the invitation doesn't offer a recording.
    expect(within(main).getByText('Start a project, then add your videos.')).toBeInTheDocument();
    expect(main.textContent).not.toMatch(/record/i);
    expect(within(main).getAllByRole('button').map((b) => b.textContent)).toEqual(['New project']);
  });

  it('empty_library_invites_new_or_record', async () => {
    recentAnswers([]);
    const onRecord = vi.fn();
    renderWithProviders(<StartScreen onRecord={onRecord} />);
    const main = screen.getByRole('main', { name: 'Your projects' });
    expect(await within(main).findByRole('heading', { name: 'No projects yet' })).toBeInTheDocument();
    expect(within(main).getByText('Start with a video or a recording of your screen.')).toBeInTheDocument();
    expect(within(main).queryByRole('list')).not.toBeInTheDocument();
    // The same two buttons, once each.
    expect(within(main).getAllByRole('button').map((b) => b.textContent)).toEqual([
      'New project',
      'Record your screen',
    ]);
    await userEvent.click(within(main).getByRole('button', { name: 'Record your screen' }));
    expect(onRecord).toHaveBeenCalledTimes(1);
    await userEvent.click(within(main).getByRole('button', { name: 'New project' }));
    expect(bridge.createProject).toHaveBeenCalledWith('Untitled project', ['landscape', 'portrait', 'square']);
  });

  it('read_failure_says_so_with_try_again', async () => {
    let answer!: (rows: ProjectRow[]) => void;
    invoke
      .mockImplementationOnce(async () => {
        throw new Error("Klipolo couldn't make its projects folder.");
      })
      .mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
    renderWithProviders(<StartScreen />);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(
      "Klipolo couldn't read your projects folder. Check the disk, then try again.",
    );
    expect(alert.querySelector('svg.ico')).not.toBeNull();
    // It says what to do, not Rust's own words.
    expect(screen.queryByText("Klipolo couldn't make its projects folder.")).not.toBeInTheDocument();
    const again = screen.getByRole('button', { name: 'Try again' });
    expect(again).toHaveClass('btn-secondary');
    // New project stays in the header, a way on.
    expect(screen.getByRole('button', { name: 'New project' })).toBeInTheDocument();

    await userEvent.tab();
    await userEvent.tab();
    expect(again).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    // Asking again: the button stays, inert with a spinner, and keeps the focus; another press does nothing.
    expect(again).toBeInTheDocument();
    expect(again).toHaveAttribute('aria-disabled', 'true');
    expect(again.querySelector('.spinner')).not.toBeNull();
    expect(again).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(invoke).toHaveBeenCalledTimes(2);

    answer(ROWS);
    await waitFor(() => expect(cards()).toHaveLength(3));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    // The focus goes on to the first card, not back to the page (moved once the cards are drawn).
    await waitFor(() => expect(card('Studio tour')).toHaveFocus());
  });

  it('one_yellow_and_no_red_or_dash', async () => {
    const check = (state: string, yellows: number) => {
      const main = screen.getByRole('main', { name: 'Your projects' });
      expect(main.querySelectorAll('.btn-primary'), state).toHaveLength(yellows);
      expect(main.querySelector('[class*="cut"]'), state).toBeNull();
      expect(main.textContent, state).not.toMatch(/[\u2013\u2014]/);
      for (const button of within(main).queryAllByRole('button')) {
        const label = button.textContent ?? '';
        // Sentence case: a capital first letter, nothing shouting after it.
        if (!button.closest('.start-grid')) expect(label, state).toMatch(/^[A-Z][a-z ]+$/);
      }
    };

    let answer!: (rows: ProjectRow[]) => void;
    invoke.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
    const first = renderWithProviders(<StartScreen onRecord={() => undefined} />);
    await screen.findByRole('status');
    check('loading', 1);
    answer(ROWS);
    await waitFor(() => expect(cards()).toHaveLength(3));
    check('list', 1);
    first.unmount();

    recentAnswers([]);
    const second = renderWithProviders(<StartScreen onRecord={() => undefined} />);
    await screen.findByText('No projects yet');
    check('empty', 1);
    second.unmount();

    recentAnswers(new Error('no'));
    renderWithProviders(<StartScreen onRecord={() => undefined} />);
    await screen.findByRole('alert');
    check('failed', 1);

    // The styles use the tokens only, and never the cut red.
    const css = readFileSync(resolve(__dirname, 'start.css'), 'utf8');
    expect(css).not.toMatch(/--color-cut/);
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i);
  });
});
