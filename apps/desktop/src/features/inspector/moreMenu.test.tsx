import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Op } from '../../bindings/Op';
import type { Snapshot } from '../../bindings/Snapshot';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The card's more menu on a title, a picture and a title template: Bring forward and Send back are on only while a
 * track of the layer's kind is there to take it, as the block menu's and ⌥↑ / ⌥↓ are.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

const LAYERS: [string, Item][] = [
  ['a title', titleItem],
  ['a picture', { ...titleItem, id: 'pic', attach: null, content: { type: 'image', assetId: 'logo' } }],
  [
    'a title template',
    {
      ...titleItem,
      id: 'card',
      attach: null,
      content: { type: 'generator', source: 'klipolo.builtin/scene.lower-third', params: { title: 'Rosa' } },
    } as unknown as Item,
  ],
];

/** The fixture with `layer` alone on the overlay track (and an empty overlay track over it when `above`), selected. */
function withLayer(layer: Item, above: boolean): Snapshot {
  const snapshot = fixtureSnapshot();
  const { tracks } = snapshot.project.timeline;
  tracks[0].items = [layer];
  if (above) tracks.splice(1, 0, { ...tracks[0], id: 'above', name: 'Overlay 2', items: [] });
  useProject.setState({ snapshot, pending: 0, apply });
  useSelection.setState({ selectedId: layer.id });
  return snapshot;
}

async function menu() {
  await userEvent.click(screen.getByRole('button', { name: 'More' }));
  const state = (name: string) => screen.getByRole('menuitem', { name }).getAttribute('aria-disabled') !== 'true';
  return { forward: state('Bring forward'), back: state('Send back'), duplicate: state('Duplicate') };
}

beforeEach(() => {
  vi.clearAllMocks();
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('the card’s layer order', () => {
  it.each(LAYERS)('%s on the only overlay track can go neither way', async (_, layer) => {
    withLayer(layer, false);
    renderWithProviders(<Inspector format="landscape" />);
    expect(await menu()).toEqual({ forward: false, back: false, duplicate: true });
  });

  it.each(LAYERS)('%s under another overlay track can be brought forward, not sent back', async (_, layer) => {
    withLayer(layer, true);
    renderWithProviders(<Inspector format="landscape" />);
    expect(await menu()).toEqual({ forward: true, back: false, duplicate: true });
  });
});
