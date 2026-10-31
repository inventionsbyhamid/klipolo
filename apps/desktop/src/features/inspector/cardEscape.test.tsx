import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Escape in a card's menu closes that menu and gives focus back to its button; the card stays, and so
 * does the selection. A second Escape, from the card itself, closes the card. The menus open in a portal, outside the
 * card, but React passes their keys up to it.
 */

vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn(() => new Promise(() => undefined)),
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

/** The fixture with a video layer (the b-roll) over clip a. */
function withVideoLayer(): Snapshot {
  const snapshot = fixtureSnapshot();
  const layer: Item = {
    ...titleItem,
    id: 'v1',
    layout: { ...titleItem.layout, rect: { x: 0.56, y: 0.56, w: 0.4, h: 0.4 }, motion: undefined },
    attach: null,
    content: { type: 'video', assetId: 'broll', sourceInUs: 0, speed: 1, volume: 1, muted: false },
  };
  snapshot.project.timeline.tracks[0].items.push(layer);
  return snapshot;
}

/** The fixture with a one-second dissolve from clip a into clip b (b starts 2 s into its file, so it has room). */
function withDissolve(): Snapshot {
  const snapshot = fixtureSnapshot();
  const [a, b] = snapshot.project.timeline.main;
  snapshot.project.timeline.main = [
    { ...a, transitionOut: { effect: 'klipolo.builtin/transition.dissolve', durationUs: 1_000_000, params: {} } },
    { ...b, sourceInUs: 2_000_000 },
  ];
  return snapshot;
}

beforeEach(() => {
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('Escape in a card’s menu closes only the menu', () => {
  it.each([
    { menu: "the clip card's More", snapshot: fixtureSnapshot, selected: 'b', card: 'broll.mp4 settings', button: 'More' },
    {
      menu: "the clip card's Speed",
      snapshot: fixtureSnapshot,
      selected: 'b',
      card: 'broll.mp4 settings',
      button: 'Speed: 1×',
    },
    { menu: "the video layer's Speed", snapshot: withVideoLayer, selected: 'v1', card: 'Video settings', button: 'Speed: 1×' },
    {
      menu: "the transition's Type",
      snapshot: withDissolve,
      selected: 'transition:a',
      card: 'Transition settings',
      button: 'Type: Dissolve',
    },
  ])('$menu', async ({ snapshot, selected, card, button }) => {
    useProject.setState({ snapshot: snapshot(), pending: 0 });
    useSelection.setState({ selectedId: selected });
    renderWithProviders(<Inspector format="landscape" />);
    const aside = screen.getByRole('complementary', { name: card });
    const trigger = within(aside).getByRole('button', { name: button });
    act(() => trigger.focus());
    await userEvent.keyboard('{Enter}');
    expect(await screen.findByRole('menu')).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    // The menu has gone; the card, the selection and the frame's size stay.
    expect(screen.queryByRole('menu')).toBeNull();
    expect(useSelection.getState().selectedId).toBe(selected);
    expect(screen.getByRole('complementary', { name: card })).toBe(aside);
    expect(trigger).toHaveFocus();
    // From the card itself, Escape closes it.
    await userEvent.keyboard('{Escape}');
    expect(useSelection.getState().selectedId).toBeNull();
    expect(screen.queryByRole('complementary', { name: card })).toBeNull();
  });
});
