import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeAll, beforeEach, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import { activateLocale } from '../../i18n';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { useToasts } from '../../state/toasts';
import { beginCommit } from '../../state/commitQueue';
import { useUi } from '../../state/ui';
import { SETTLE_MS } from '../../state/bursts';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderOpen } from '../../test/open';
import { editor, heard, rustForText, titleBox } from '../../test/textEdit';
import { Inspector } from '../inspector/Inspector';
import { BOX_COLOR } from '../inspector/inspectorOps';
import { Stage } from './Stage';
import { useTextEdit } from './textEdit';
import { queueCommit } from './toolbar/toolOps';

/**
 * Words typed on the canvas, or in the inspector's field, and not yet saved as the project closes: the edit waits its
 * turn behind a change to the same title still on its way, and is built on it, never putting it back. The inspector's
 * style presses do the same, close or not.
 */

beforeAll(() => {
  // jsdom has no layout: give the stage an 800×450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(async () => {
  await activateLocale('en');
  heard.reset();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useUi.setState({ previewFormat: null });
  useToasts.setState({ toasts: [] });
  useTextEdit.setState({ session: null, closing: null });
});

const titleIn = (project: { timeline: { tracks: { items: Item[] }[] } }) =>
  project.timeline.tracks.flatMap((t) => t.items).find((i) => i.id === 'title')!;

/** Rust's project: each edit's content patch lands on its title, and the answer to `label` (the colour's) is held. */
function rustHoldingColour(label = 'text.color.undo') {
  const rust = structuredClone(fixtureSnapshot());
  let release!: () => void;
  const held = new Promise<void>((r) => (release = r));
  rustForText(async (tx) => {
    if (tx.label === label) await held;
    for (const op of tx.ops)
      if (op.type === 'updateItem') Object.assign(titleIn(rust.project), op.patch as Partial<Item>);
    return fixturePatch(rust);
  });
  return { rust, release };
}

/** Another control changes the title's colour (the whole content, as text tools send it); its answer is on its way. */
function colourOnItsWay() {
  const content = titleIn(useProject.getState().snapshot!.project).content as Extract<
    Item['content'],
    { type: 'text' }
  >;
  void queueCommit('text.color.undo', () => [
    {
      type: 'updateItem',
      id: 'title',
      patch: { content: { ...content, style: { ...content.style, color: '#FF0000' } } },
    },
  ]);
}

/** Closes the project while the colour's answer is held, then lets it land. */
async function closeThenLand(release: () => void) {
  let closed!: Promise<void>;
  act(() => void (closed = useProject.getState().close()));
  release();
  await act(() => closed);
}

it('words typed on the canvas, finished as the project closes, go in after the title’s colour change on its way, built on it', async () => {
  const { rust, release } = rustHoldingColour();
  renderOpen(<Stage />);
  await userEvent.dblClick(titleBox());
  await userEvent.type(editor(), ' now');
  colourOnItsWay();
  await closeThenLand(release);
  expect(heard.applied.map((tx) => tx.label)).toEqual(['text.color.undo', 'text.edit.undo']);
  // Both kept: the words, and the colour they were typed over.
  expect(titleIn(rust.project).content).toMatchObject({
    text: 'Set up billing now',
    style: { color: '#FF0000' },
  });
});

it('words typed in the inspector’s field and not left go in, as the project closes, after the colour change on its way', async () => {
  const { rust, release } = rustHoldingColour();
  useSelection.setState({ selectedId: 'title' });
  renderOpen(<Inspector format="landscape" />);
  await userEvent.type(screen.getByRole('textbox', { name: 'Text' }), ' too');
  colourOnItsWay();
  await closeThenLand(release);
  expect(heard.applied.map((tx) => tx.label)).toEqual(['text.color.undo', 'text.edit.undo']);
  expect(titleIn(rust.project).content).toMatchObject({
    text: 'Set up billing too',
    style: { color: '#FF0000' },
  });
});

it('a one-shot press (Background: Box) made while a commit is on its way goes in, in its turn, as the project closes', async () => {
  const { rust } = rustHoldingColour();
  useSelection.setState({ selectedId: 'title' });
  renderOpen(<Inspector format="landscape" />);
  // Another commit is on its way (a slider let go just before): the press waits its turn; closing never drops it.
  const end = beginCommit();
  await userEvent.click(screen.getByRole('radio', { name: 'Box' }));
  expect(heard.applied).toEqual([]);
  await closeThenLand(end);
  expect(heard.applied.map((tx) => tx.label)).toEqual(['text.background.undo']);
  expect(titleIn(rust.project).content).toMatchObject({ style: { background: BOX_COLOR, shadow: false } });
});

it('a canvas edit with nothing to commit settles as the project closes behind a commit on its way: the title shows again (Minor C)', async () => {
  rustForText();
  renderOpen(<Stage />);
  await userEvent.dblClick(titleBox());
  const end = beginCommit();
  await closeThenLand(end);
  expect(heard.calls).toEqual(['hide title', 'show']);
});

it('a canvas edit whose title went meanwhile puts its words on the clipboard as the project closes behind a commit (Minor C)', async () => {
  const write = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: write }, configurable: true });
  rustForText();
  renderOpen(<Stage />);
  await userEvent.dblClick(titleBox());
  await userEvent.type(editor(), ' now');
  // A commit is on its way as a chat change takes the title away while it is typed.
  const end = beginCommit();
  act(() =>
    useProject.setState((s) => {
      const next = structuredClone(s.snapshot!);
      for (const track of next.project.timeline.tracks)
        track.items = track.items.filter((i) => i.id !== 'title');
      return { snapshot: next };
    }),
  );
  await closeThenLand(end);
  expect(write).toHaveBeenCalledWith('Set up billing now');
});

/*
 * A style press is built from the title as the project has it at its turn, not as the press's render saw it: a change
 * to the same title on its way, or still settling, is kept, never put back.
 */
const labels = () => heard.applied.map((tx) => tx.label);

it('a larger size, then Box before the size settles: the title ends a size larger with its box, both kept', async () => {
  const { rust } = rustHoldingColour();
  useSelection.setState({ selectedId: 'title' });
  renderOpen(<Inspector format="landscape" />);
  // A key on the size slider settles as a burst; a click that doesn't move focus (as in WKWebView) goes in meanwhile.
  fireEvent.keyDown(screen.getByRole('slider', { name: 'Size' }), { key: 'ArrowRight' });
  fireEvent.click(screen.getByRole('radio', { name: 'Box' }));
  await act(() => new Promise((r) => setTimeout(r, SETTLE_MS + 50)));
  await waitFor(() => expect(heard.applied).toHaveLength(2));
  expect(labels()).toEqual(['text.background.undo', 'text.size.undo']);
  // The fixture title is 72: one step up is 73, and Box gave it its box.
  expect(titleIn(rust.project).content).toMatchObject({ style: { size: 73, background: BOX_COLOR } });
});

it('Box pressed while a colour change to the title is on its way keeps the colour', async () => {
  const { rust, release } = rustHoldingColour();
  useSelection.setState({ selectedId: 'title' });
  renderOpen(<Inspector format="landscape" />);
  colourOnItsWay();
  await userEvent.click(screen.getByRole('radio', { name: 'Box' }));
  expect(labels()).toEqual(['text.color.undo']);
  await act(async () => release());
  await waitFor(() => expect(heard.applied).toHaveLength(2));
  expect(labels()).toEqual(['text.color.undo', 'text.background.undo']);
  expect(titleIn(rust.project).content).toMatchObject({ style: { color: '#FF0000', background: BOX_COLOR } });
});

it('a text colour picked while Box is on its way keeps the box', async () => {
  const { rust, release } = rustHoldingColour('text.background.undo');
  useSelection.setState({ selectedId: 'title' });
  renderOpen(<Inspector format="landscape" />);
  await userEvent.click(screen.getByRole('radio', { name: 'Box' }));
  await userEvent.click(within(screen.getByRole('radiogroup', { name: 'Text color' })).getByRole('radio', { name: 'Yellow' }));
  expect(labels()).toEqual(['text.background.undo']);
  await act(async () => release());
  await waitFor(() => expect(heard.applied).toHaveLength(2));
  expect(labels()).toEqual(['text.background.undo', 'text.color.undo']);
  expect(titleIn(rust.project).content).toMatchObject({ style: { color: '#FFD02E', background: BOX_COLOR } });
});
