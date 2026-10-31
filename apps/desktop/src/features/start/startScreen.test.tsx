import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectRow } from '../../bindings/ProjectRow';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

// `projects_recent` goes through the real bridge to this `invoke`; creating and opening go through the project bridge.
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('../../bridge/commands', () => ({
  bridge: { createProject: vi.fn(), openProject: vi.fn(), snapshot: vi.fn() },
  mediaUrl: (path: string) => `asset://localhost${path}`,
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
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

const cards = () => within(screen.getByRole('list')).getAllByRole('button');
const card = (name: string) => cards().find((c) => c.textContent?.includes(name))!;

beforeEach(() => {
  vi.spyOn(Date, 'now').mockReturnValue(NOW);
  useProject.setState({ snapshot: null, pending: 0 });
  vi.mocked(bridge.createProject).mockResolvedValue(fixtureSnapshot());
  vi.mocked(bridge.openProject).mockResolvedValue(fixtureSnapshot());
});

afterEach(() => {
  vi.restoreAllMocks();
  invoke.mockReset();
  vi.mocked(bridge.createProject).mockReset();
  vi.mocked(bridge.openProject).mockReset();
});

describe('StartScreen', () => {
  it('lists_projects_with_name_opened_time_and_duration', async () => {
    let answer!: (rows: ProjectRow[]) => void;
    invoke.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    renderWithProviders(<StartScreen />);
    const main = screen.getByRole('main', { name: 'Your projects' });
    expect(within(main).getByRole('heading', { name: 'Your projects' })).toBeInTheDocument();
    // Loading: a spinner and one line.
    expect(within(main).getByRole('status')).toHaveTextContent('Loading your projects');
    expect(main.querySelector('.spinner')).not.toBeNull();
    expect(invoke).toHaveBeenCalledWith('projects_recent', undefined);

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
    // The header's two actions.
    expect(within(main).getByRole('button', { name: 'New project' })).toBeInTheDocument();
    expect(within(main).getByRole('button', { name: 'Record your screen' })).toBeInTheDocument();
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

    await userEvent.click(card('Kiln vlog'));
    expect(bridge.openProject).toHaveBeenCalledWith('/Users/me/Movies/Klipolo/Kiln vlog.klipolo');
    await waitFor(() => expect(useProject.getState().pending).toBe(0));
    expect(useProject.getState().snapshot).not.toBeNull();

    // From the keyboard: Tab to a card, Enter opens it.
    card('Studio tour').focus();
    await userEvent.keyboard('{Enter}');
    expect(bridge.openProject).toHaveBeenLastCalledWith('/Users/me/Movies/Klipolo/Studio tour.klipolo');
    expect(bridge.openProject).toHaveBeenCalledTimes(2);
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
    const { unmount } = renderWithProviders(<StartScreen onRecord={onRecord} />);
    await waitFor(() => expect(cards()).toHaveLength(3));
    const record = screen.getByRole('button', { name: 'Record your screen' });
    // The record glyph, in the button's own ink.
    expect(record.querySelector('svg.ico')).not.toBeNull();
    await userEvent.click(record);
    expect(onRecord).toHaveBeenCalledTimes(1);
    expect(bridge.createProject).not.toHaveBeenCalled();
    unmount();

    // Until the recorder is wired, the button does nothing (and doesn't throw).
    recentAnswers([]);
    renderWithProviders(<StartScreen />);
    await userEvent.click(await screen.findByRole('button', { name: 'Record your screen' }));
    expect(bridge.createProject).not.toHaveBeenCalled();
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
    recentAnswers(new Error("Klipolo couldn't make its projects folder."), ROWS);
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

    await userEvent.click(again);
    await waitFor(() => expect(cards()).toHaveLength(3));
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
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
    const first = renderWithProviders(<StartScreen />);
    check('loading', 0);
    answer(ROWS);
    await waitFor(() => expect(cards()).toHaveLength(3));
    check('list', 1);
    first.unmount();

    recentAnswers([]);
    const second = renderWithProviders(<StartScreen />);
    await screen.findByText('No projects yet');
    check('empty', 1);
    second.unmount();

    recentAnswers(new Error('no'));
    renderWithProviders(<StartScreen />);
    await screen.findByRole('alert');
    check('failed', 1);

    // The styles use the tokens only, and never the cut red.
    const css = readFileSync(resolve(__dirname, 'start.css'), 'utf8');
    expect(css).not.toMatch(/--color-cut/);
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i);
  });
});
