import { act, fireEvent, renderHook, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders, TestProviders } from '../../test/render';

/*
 * Control Command I folds chat or opens it, and Control Command S folds the library or opens it: from a field too, once
 * per press, never while a menu or dialog has the keyboard.
 */

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
    preview: vi.fn(() => Promise.resolve()),
    previewClear: vi.fn(() => Promise.resolve()),
  },
}));
vi.mock('../media/useImportMedia', async (load) => ({
  ...(await load<typeof import('../media/useImportMedia')>()),
  importFiles: vi.fn(() => Promise.resolve()),
}));
const { importFiles } = await import('../media/useImportMedia');
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { useChatPanel } = await import('../../state/chatPanel');
const { useLibraryPanel } = await import('../../state/libraryPanel');
const { useEditorShortcuts } = await import('./useEditorShortcuts');
const { usePanelCommands } = await import('./usePanelCommands');

function Harness() {
  useEditorShortcuts();
  return <input aria-label="Project name" />;
}

const controlCommand = (target: Element | Window, key: string, extra: Record<string, unknown> = {}) =>
  fireEvent.keyDown(target, { key, code: `Key${key.toUpperCase()}`, ctrlKey: true, metaKey: true, ...extra });

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useUi.setState({ chatFocus: 0 });
  useChatPanel.setState({ collapsed: false });
  useLibraryPanel.setState({ collapsed: false });
  vi.mocked(importFiles).mockClear();
});

describe('the panel keys', () => {
  it('work while typing in a field, and never import or type', async () => {
    renderWithProviders(<Harness />);
    const field = screen.getByRole('textbox', { name: 'Project name' });
    await userEvent.click(field);
    await userEvent.keyboard('{Control>}{Meta>}i{/Meta}{/Control}');
    expect(useChatPanel.getState().collapsed).toBe(true);
    // Opening chat puts the cursor in its composer, as Command K does.
    await userEvent.keyboard('{Control>}{Meta>}i{/Meta}{/Control}');
    expect(useUi.getState().chatFocus).toBe(1);
    await userEvent.keyboard('{Control>}{Meta>}s{/Meta}{/Control}');
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    await userEvent.keyboard('{Control>}{Meta>}s{/Meta}{/Control}');
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    expect(importFiles).not.toHaveBeenCalled();
    expect(field).toHaveValue('');
  });

  it("a held key folds once; another layout's key matches by the key it sits on", () => {
    renderWithProviders(<Harness />);
    controlCommand(window, 'i');
    controlCommand(window, 'i', { repeat: true });
    expect(useChatPanel.getState().collapsed).toBe(true);
    controlCommand(window, 'ı', { code: 'KeyI' });
    expect(useUi.getState().chatFocus).toBe(1);
    controlCommand(window, 's');
    controlCommand(window, 's', { repeat: true });
    expect(useLibraryPanel.getState().collapsed).toBe(true);
  });

  it('wait while a menu has the keyboard, and leave Shift and Option alone', () => {
    renderWithProviders(
      <>
        <Harness />
        <div role="menu">
          <button type="button">Rename</button>
        </div>
      </>,
    );
    const item = screen.getByRole('button', { name: 'Rename' });
    controlCommand(item, 'i');
    controlCommand(item, 's');
    controlCommand(window, 'i', { shiftKey: true });
    controlCommand(window, 's', { altKey: true });
    expect(useChatPanel.getState().collapsed).toBe(false);
    expect(useLibraryPanel.getState().collapsed).toBe(false);
  });

  it('Command B folds and opens the library too, but never while typing in a field', async () => {
    renderWithProviders(<Harness />);
    fireEvent.keyDown(window, { key: 'b', code: 'KeyB', metaKey: true });
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    fireEvent.keyDown(window, { key: 'b', code: 'KeyB', metaKey: true });
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    const field = screen.getByRole('textbox', { name: 'Project name' });
    await userEvent.click(field);
    await userEvent.keyboard('{Meta>}b{/Meta}');
    expect(useLibraryPanel.getState().collapsed).toBe(false);
  });

  it('are commands that say what a press does, with their keys', () => {
    const { result } = renderHook(() => usePanelCommands(), { wrapper: TestProviders });
    const byId = (id: string) => result.current.find((c) => c.id === id)!;
    expect(byId('toggle-chat')).toMatchObject({ label: 'Collapse chat', shortcut: '⌃⌘I', enabled: true });
    expect(byId('toggle-side-panel')).toMatchObject({ label: 'Collapse library', shortcut: '⌘B', enabled: true });
    act(() => byId('toggle-chat').run());
    act(() => byId('toggle-side-panel').run());
    expect(byId('toggle-chat').label).toBe('Open chat');
    expect(byId('toggle-side-panel').label).toBe('Open library');
  });
});
