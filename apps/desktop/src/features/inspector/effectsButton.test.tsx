import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Effects on the cards: every card of a clip or of a layer that draws has an Effects button ("Effects 2" when there are
 * some) that opens the stack, and the stack follows each change as it lands. A sound has none.
 */

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
vi.mock('../../bridge/fonts', () => ({
  fonts: {
    bundled: vi.fn(() => Promise.resolve([{ family: 'Inter', weight: 400, italic: false }])),
    face: vi.fn(() => Promise.reject(new Error('no faces in tests'))),
    faceOf: vi.fn(() => Promise.reject(new Error('no faces in tests'))),
    row: vi.fn(() => Promise.resolve(null)),
    google: vi.fn(() => Promise.resolve({ enabled: true, consented: false })),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { engineApply, fx, onClipA, withLayers } = await import('../../test/inspector');
const { Inspector } = await import('./Inspector');

const lastTx = () => vi.mocked(bridge.apply).mock.calls.at(-1)![0];

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});

describe('Effects on the cards', () => {
  it('a layer’s card opens its stack from Effects, with its count, and follows each change', async () => {
    withLayers({ id: 'logo-layer', content: { type: 'image', assetId: 'logo' }, effects: [fx('fx', 'blur')] });
    useSelection.setState({ selectedId: 'logo-layer' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Effects 1' }));
    const stack = screen.getByRole('dialog', { name: 'Effects' });
    expect(within(stack).getByRole('switch', { name: 'Blur' })).toBeChecked();
    await userEvent.click(within(stack).getByRole('button', { name: 'Add effect' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Glow' }));
    expect(lastTx().ops[0]).toMatchObject({ type: 'addEffect', target: { type: 'item', id: 'logo-layer' } });
    // The new effect shows in the open stack, and the button counts it.
    await waitFor(() => expect(within(stack).getByRole('switch', { name: 'Glow' })).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Effects 2' })).toBeInTheDocument();
  });

  it('Escape in the stack closes only the stack, and the card stays', async () => {
    withLayers({ id: 'logo-layer', content: { type: 'image', assetId: 'logo' }, effects: [fx('fx', 'blur')] });
    useSelection.setState({ selectedId: 'logo-layer' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Effects 1' }));
    expect(screen.getByRole('dialog', { name: 'Effects' })).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Effects' })).toBeNull());
    expect(screen.getByRole('complementary', { name: 'Image settings' })).toBeInTheDocument();
    expect(useSelection.getState().selectedId).toBe('logo-layer');
  });

  it('the clip card has Effects for the clip', async () => {
    onClipA([fx('fx', 'vignette')]);
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Effects 1' }));
    await userEvent.click(screen.getByRole('switch', { name: 'Vignette' }));
    expect(lastTx()).toEqual({
      label: 'effect.off.undo',
      params: { effect: 'klipolo.builtin/effect.vignette' },
      ops: [{ type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'fx', patch: { enabled: false } }],
    });
  });

  it('text, video and title template cards have Effects too; a sound’s has none', async () => {
    withLayers(
      { id: 'v1', content: { type: 'video', assetId: 'broll', sourceInUs: 0, speed: 1, volume: 1, muted: false } },
      {
        id: 'lower',
        content: { type: 'generator', source: 'klipolo.builtin/scene.lower-third', params: {} },
      },
    );
    for (const id of ['title', 'v1', 'lower']) {
      useSelection.setState({ selectedId: id });
      const { unmount } = renderWithProviders(<Inspector format="landscape" />);
      // No effects yet: just "Effects".
      expect(screen.getByRole('button', { name: 'Effects' })).toBeInTheDocument();
      unmount();
    }
    useSelection.setState({ selectedId: 'song' });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Effects/ })).toBeNull();
  });
});

describe('the Effects popover', () => {
  it('lines up with the card’s right edge, at the card’s width, never taller than the room it has', async () => {
    withLayers({ id: 'logo-layer', content: { type: 'image', assetId: 'logo' }, effects: [fx('fx', 'blur')] });
    useSelection.setState({ selectedId: 'logo-layer' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Effects 1' }));
    expect(screen.getByRole('dialog', { name: 'Effects' })).toHaveAttribute('data-align', 'end');
    const css = readFileSync(resolve(__dirname, '../../styles/app.css'), 'utf8');
    const pop = /\.fx-pop \{([^}]*)\}/.exec(css)![1];
    expect(pop).toMatch(/width: calc\(var\(--inspector-w\)/);
    expect(pop).toMatch(/max-height: min\([^;]*var\(--radix-popover-content-available-height\)\)/);
  });
});
