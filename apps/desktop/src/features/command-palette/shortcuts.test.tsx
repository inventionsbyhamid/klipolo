import { act, fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useRange } = await import('../../state/range');
const { useLoop } = await import('../../state/loop');
const { useSelection } = await import('../../state/selection');
const { Timeline } = await import('../timeline/Timeline');
const { useEditorShortcuts } = await import('./useEditorShortcuts');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

/** The timeline and its keys, beside a text field, a composer, an open dialog and an open menu. */
function Editor() {
  useEditorShortcuts();
  return (
    <>
      <input aria-label="Name" />
      <textarea aria-label="Message" />
      <div role="dialog" aria-label="Export">
        <button type="button">Cancel</button>
      </div>
      <div role="menu" aria-label="Project">
        <div role="menuitem" tabIndex={-1}>
          Rename
        </div>
      </div>
      <Timeline />
    </>
  );
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: 'title' });
  useRange.setState({ range: null });
  // Inside the title (1 to 4 s) and the first clip: S would split one of them.
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
});

describe("the editor's keys leave fields, dialogs and menus alone", () => {
  it('I, S and Delete typed in a text field or the composer edit nothing', async () => {
    renderWithProviders(<Editor />);
    const fields = [screen.getByRole('textbox', { name: 'Name' }), screen.getByRole('textbox', { name: 'Message' })];
    for (const field of fields)
      for (const key of ['i', 'o', 's', 'Delete', 'Backspace', ' ', 'j', 'l', '1']) fireEvent.keyDown(field, { key });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useRange.getState().range).toBeNull();
    expect(useSelection.getState().selectedId).toBe('title');
    expect(usePlayback.getState().playing).toBe(false);
  });

  it('Escape clears the selection, but not while typing in a field', () => {
    renderWithProviders(<Editor />);
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), { key: 'Escape' });
    expect(useSelection.getState().selectedId).toBe('title');
    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(useSelection.getState().selectedId).toBeNull();
  });

  it('keys pressed in an open dialog or menu do nothing in the editor', async () => {
    renderWithProviders(<Editor />);
    const inDialog = screen.getByRole('button', { name: 'Cancel' });
    const inMenu = screen.getByRole('menuitem', { name: 'Rename' });
    for (const target of [inDialog, inMenu])
      for (const key of ['s', 'Delete', 'Escape', 'i', 'Home']) fireEvent.keyDown(target, { key });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useSelection.getState().selectedId).toBe('title');
    expect(useRange.getState().range).toBeNull();
    expect(usePlayback.getState().playheadUs).toBe(2_000_000);
  });

  it('the same keys outside them do edit: S splits the selected layer', async () => {
    renderWithProviders(<Editor />);
    fireEvent.keyDown(document.body, { key: 's' });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });
});

describe('playback keys', () => {
  it('⌘L turns Loop on and off without playing; typed in a field it does nothing', () => {
    useLoop.setState({ on: false });
    renderWithProviders(<Editor />);
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Name' }), { key: 'l', metaKey: true });
    expect(useLoop.getState().on).toBe(false);
    fireEvent.keyDown(document.body, { key: 'l', metaKey: true });
    expect(useLoop.getState().on).toBe(true);
    expect(usePlayback.getState().playing).toBe(false);
    fireEvent.keyDown(document.body, { key: 'l', metaKey: true });
    expect(useLoop.getState().on).toBe(false);
  });
});
