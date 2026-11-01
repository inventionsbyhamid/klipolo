import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import { Toaster } from '../../design/Toaster';
import { activateLocale } from '../../i18n';
import { safeRect } from '../../lib/safeAreas';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { useToasts } from '../../state/toasts';
import { useUi } from '../../state/ui';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { editor, heard, rustForText as rust, titleBox as layer } from '../../test/textEdit';
import { Inspector } from '../inspector/Inspector';
import { AddTitleMenu } from './AddTitleMenu';
import { Stage } from './Stage';
import { useTextEdit } from './textEdit';
import { useAddText } from './useAddText';

beforeAll(() => {
  // jsdom has no layout: give the stage an 800×450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(async () => {
  await activateLocale('en');
  heard.reset();
  // The title "Set up billing" shows from 1 s to 4 s.
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useUi.setState({ previewFormat: null });
  useToasts.setState({ toasts: [] });
  useTextEdit.setState({ session: null, closing: null });
  rust();
});

describe('on-canvas text', () => {
  it('double-click edits in place, the engine hides that layer until the edit lands, one Edit text step', async () => {
    renderWithProviders(<Stage />);
    await userEvent.dblClick(layer());
    expect(heard.editing).toEqual(['title']);
    const box = editor();
    expect(box).toHaveFocus();
    expect(box.selectionStart).toBe('Set up billing'.length);
    // Its box gives way to the editor while it is typed.
    expect(screen.queryByRole('button', { name: /^(Select |Text: )Set up billing/ })).not.toBeInTheDocument();
    await userEvent.type(editor(), ' now');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(heard.editing).toEqual(['title', null]));
    expect(heard.calls, 'shown again only after the commit answered').toEqual([
      'hide title',
      'apply',
      'show',
    ]);
    expect(heard.applied).toHaveLength(1);
    expect(heard.applied[0].label).toBe('text.edit.undo');
    expect(heard.applied[0].ops[0]).toMatchObject({
      type: 'updateItem',
      id: 'title',
      patch: { content: { type: 'text', text: 'Set up billing now' } },
    });
  });

  it('Return starts a new line; a click outside finishes', async () => {
    renderWithProviders(<Stage />);
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), '{Enter}there');
    expect(editor()).toHaveValue('Set up billing\nthere');
    expect(heard.applied).toHaveLength(0);
    await userEvent.click(screen.getByTestId('stage'));
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(useTextEdit.getState().session).toBeNull();
  });

  it('Return on the selected title begins, and the editor’s keys never reach the shortcuts', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Stage />);
    layer().focus();
    await userEvent.keyboard('{Enter}');
    expect(editor()).toHaveFocus();
    const shortcut = vi.fn();
    window.addEventListener('keydown', shortcut);
    await userEvent.keyboard(' jk{Backspace}');
    window.removeEventListener('keydown', shortcut);
    expect(shortcut).not.toHaveBeenCalled();
    expect(editor()).toHaveValue('Set up billing j');
    expect(usePlayback.getState().playing).toBe(false);
  });

  it('composition keeps Return and Escape, and the length cap waits for compositionend', async () => {
    renderWithProviders(<Stage />);
    await userEvent.dblClick(layer());
    const box = editor();
    fireEvent.compositionStart(box);
    fireEvent.keyDown(box, { key: 'Escape', isComposing: true });
    fireEvent.keyDown(box, { key: 'Enter', isComposing: true });
    fireEvent.change(box, { target: { value: 'x'.repeat(520) } });
    expect(box.value.length, 'never rewritten mid-composition').toBe(520);
    fireEvent.compositionEnd(box);
    expect(box.value.length).toBe(500);
    expect(useTextEdit.getState().session).not.toBeNull();
    expect(heard.applied).toHaveLength(0);
  });

  it('a draft typed and finished is one Add text step; an empty draft commits nothing', async () => {
    const add = { current: null as null | (() => Promise<void>) };
    function Adder() {
      add.current = useAddText();
      return null;
    }
    renderWithProviders(
      <>
        <Stage />
        <Adder />
      </>,
    );
    await act(() => add.current!());
    expect(heard.applied, 'a draft, not yet a layer').toHaveLength(0);
    const box = editor();
    expect(box.value.slice(box.selectionStart, box.selectionEnd)).toBe('Your text');
    await userEvent.keyboard('Budgeting 101{Escape}');
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(heard.applied[0].label).toBe('text.add');
    expect(heard.applied[0].ops[0]).toMatchObject({ type: 'addItem', attach: true });
    expect(JSON.stringify(heard.applied[0].ops[0])).toContain('Budgeting 101');
    expect(heard.editing, 'a draft was never in the preview').toEqual([]);

    await act(() => add.current!());
    await userEvent.clear(editor());
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(useTextEdit.getState().closing).toBeNull());
    expect(heard.applied).toHaveLength(1);
  });

  it('a title added in a 16:9 and 9:16 video goes inside what the apps leave clear in 9:16, wherever it is typed', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'portrait' });
    const add = { current: null as null | (() => Promise<void>) };
    function Adder() {
      add.current = useAddText();
      return null;
    }
    renderWithProviders(
      <>
        <Stage />
        <Adder />
      </>,
    );
    await act(() => add.current!());
    await userEvent.keyboard('Rosa’s studio{Escape}');
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    const { layout } = (heard.applied[0].ops[0] as unknown as { item: Item }).item;
    const clear = safeRect('portrait', 'all')!;
    const portrait = layout.overrides.portrait!;
    expect(portrait.x).toBeGreaterThanOrEqual(clear.x);
    expect(portrait.y).toBeGreaterThanOrEqual(clear.y);
    expect(portrait.x + portrait.w).toBeLessThanOrEqual(clear.x + clear.w);
    expect(portrait.y + portrait.h).toBeLessThanOrEqual(clear.y + clear.h);
    // Its own place in 1:1 too, and centred at the top of the 16:9 frame.
    expect(layout.overrides.square).toBeDefined();
    expect(layout.rect.x + layout.rect.w / 2).toBeCloseTo(0.5, 9);
  });

  it('a title finished and a new one begun at once: the first one’s hand-back never takes the new box’s focus', async () => {
    const add = { current: null as null | (() => Promise<void>) };
    function Adder() {
      add.current = useAddText();
      return null;
    }
    renderWithProviders(
      <>
        <Stage />
        <Adder />
      </>,
    );
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), ' now');
    await userEvent.keyboard('{Escape}');
    // Before the first edit has handed the focus back to its layer (no frames come in tests: it waits its cap).
    await act(() => add.current!());
    await act(() => new Promise((resolve) => setTimeout(resolve, 300)));
    expect(editor()).toHaveFocus();
    await userEvent.keyboard('Rosa{Escape}');
    await waitFor(() => expect(heard.applied).toHaveLength(2));
    expect(heard.applied[1].label).toBe('text.add');
    expect(JSON.stringify(heard.applied[1].ops[0])).toContain('Rosa');
  });

  it('Add title, Text, opens the new box with the cursor in it, as the menu closes', async () => {
    renderWithProviders(
      <>
        <Stage />
        <AddTitleMenu />
      </>,
    );
    // The person was on the title's box when they opened the menu.
    act(() => layer().focus());
    await userEvent.click(screen.getByRole('button', { name: 'Add title' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Text' }));
    await waitFor(() => expect(editor()).toHaveFocus());
    // Once the closing menu has settled, the cursor is still in the new box, a draft.
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(editor()).toHaveFocus();
    expect(useTextEdit.getState().session?.draft).toBeTruthy();
    expect(heard.applied).toHaveLength(0);
  });

  it('emptying a title removes it in one step', async () => {
    renderWithProviders(<Stage />);
    await userEvent.dblClick(layer());
    await userEvent.clear(editor());
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(heard.applied).toEqual([
      expect.objectContaining({ label: 'text.remove.undo', ops: [{ type: 'removeItem', id: 'title' }] }),
    ]);
  });

  it('playing finishes the edit first', async () => {
    renderWithProviders(<Stage />);
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), '!');
    act(() => usePlayback.getState().toggle());
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(heard.applied[0].ops[0]).toMatchObject({ patch: { content: { text: 'Set up billing!' } } });
  });

  it('the inspector’s field and the canvas edit one draft', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(
      <>
        <Stage />
        <Inspector format="landscape" />
      </>,
    );
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), ' there');
    const field = screen.getByRole('textbox', { name: 'Text' });
    expect(field).toHaveValue('Set up billing there');
    await userEvent.type(field, '!');
    expect(editor()).toHaveValue('Set up billing there!');
    expect(heard.applied).toHaveLength(0);
    // Return in the field finishes the shared edit, keeping the words: one commit.
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(heard.applied[0].ops[0]).toMatchObject({ patch: { content: { text: 'Set up billing there!' } } });
  });

  it('Escape keeps the words typed on the canvas, and puts the saved words back from the inspector’s field', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(
      <>
        <Stage />
        <Inspector format="landscape" />
      </>,
    );
    // From the inspector's field, while the canvas types the same words: they go back, nothing is committed.
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), ' there');
    const field = screen.getByRole('textbox', { name: 'Text' });
    await userEvent.click(field);
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(useTextEdit.getState().closing).toBeNull());
    expect(useTextEdit.getState().session).toBeNull();
    expect(heard.applied).toHaveLength(0);
    expect(screen.getByRole('textbox', { name: 'Text' })).toHaveValue('Set up billing');
    // On the canvas: the words typed stay, one commit.
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), ' now');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(heard.applied[0].ops[0]).toMatchObject({ patch: { content: { text: 'Set up billing now' } } });
  });

  it('an inspector press that finishes the edit builds on the new words, never putting the old ones back', async () => {
    // Rust takes its time with the edit, so a colour is picked while it is on its way; each edit lands for real.
    rust(async (tx) => {
      await new Promise((resolve) => setTimeout(resolve, 40));
      const next = structuredClone(useProject.getState().snapshot!);
      const op = tx.ops[0] as unknown as { id: string; patch: { content: unknown } };
      const items = next.project.timeline.tracks[0].items;
      items[0] = { ...items[0], content: op.patch.content as (typeof items)[0]['content'] };
      return fixturePatch(next);
    });
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(
      <>
        <Inspector format="landscape" />
        <Stage />
      </>,
    );
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), ' now');
    await userEvent.click(within(screen.getByRole('radiogroup', { name: 'Text color' })).getByRole('radio', { name: 'Yellow' }));
    await waitFor(() => expect(heard.applied).toHaveLength(2));
    expect(heard.applied.map((tx) => tx.label)).toEqual(['text.edit.undo', 'text.color.undo']);
    expect(heard.applied[1].ops[0]).toMatchObject({
      patch: { content: { text: 'Set up billing now', style: { color: '#FFD02E' } } },
    });
    // Both land, and the edit is over, before the next test starts.
    await waitFor(() => expect(useProject.getState().pending).toBe(0));
    await waitFor(() => expect(useTextEdit.getState().closing).toBeNull());
  });

  it('a commit the video moved under keeps the words, said once', async () => {
    const write = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: write }, configurable: true });
    rust(() => Promise.reject({ code: 'project.moved', params: {} }));
    const said: string[] = [];
    const off = useToasts.subscribe((t) => said.push(...t.toasts.map((toast) => toast.text)));
    renderWithProviders(
      <>
        <Stage />
        <Toaster />
      </>,
    );
    await userEvent.dblClick(layer());
    await userEvent.type(editor(), ' now');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(write).toHaveBeenCalledWith('Set up billing now'));
    expect(
      await screen.findAllByText('The video changed while you typed. Your words are on the clipboard.'),
    ).toHaveLength(1);
    off();
    expect(said, 'the moved toast of its own is never raised').not.toContain('The video changed. Try again.');
  });
});
