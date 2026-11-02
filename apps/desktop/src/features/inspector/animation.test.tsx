import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Motion } from '../../bindings/Motion';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { animationOf, emphasisOf, withAnimation, withEmphasis } from './inspectorOps';

/*
 * A layer card's Animation row: how it comes in (In), goes out (Out) and moves while it shows (Loop), as chat's
 * `animate` sets them, so the person sees and changes what chat did. Each pick is one undo step.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { Inspector } = await import('./Inspector');

const use = (word: string, durationUs: number, params: Record<string, string | number> = {}) => ({
  preset: `klipolo.builtin/motion.${word}`,
  params,
  durationUs,
});

/** A picture layer moving as chat's `animate` leaves it: bouncing in from nothing, sliding out right, pulsing. */
const chatMotion: Motion = {
  enter: use('bounce', 600_000),
  exit: use('slide', 300_000, { from: 'right' }),
  loop: use('pulse', 800_000, { strength: 1.5 }),
};
const picture: Item = {
  ...titleItem,
  id: 'heart',
  content: { type: 'image', assetId: 'logo' },
  layout: { ...titleItem.layout, motion: chatMotion },
};

beforeEach(() => {
  vi.clearAllMocks();
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.tracks[0].items.push(picture);
  useProject.setState({ snapshot, pending: 0, apply });
  useSelection.setState({ selectedId: 'heart' });
});

describe('the Animation row', () => {
  it('reads every way chat brings a layer in, takes it out and moves it', () => {
    const at = (motion: Motion) => ({
      enter: animationOf(motion, 'enter'),
      exit: animationOf(motion, 'exit'),
      loop: emphasisOf(motion),
    });
    expect(at(chatMotion)).toEqual({ enter: 'bounce', exit: 'slideRight', loop: 'pulse' });
    expect(
      at({ enter: use('slide', 400_000, { from: 'left' }), exit: use('slide', 300_000, { from: 'top' }) }),
    ).toEqual({ enter: 'slideRight', exit: 'slideUp', loop: 'none' });
    expect(at({ enter: use('blur-in', 500_000), loop: use('wiggle', 800_000, { strength: 1.5 }) })).toEqual({
      enter: 'blur',
      exit: 'none',
      loop: 'wiggle',
    });
    // One the card doesn't offer shows as it is.
    expect(at({ loop: use('spin', 800_000) }).loop).toBeNull();
  });

  it('writes what chat writes for the same pick', () => {
    const layout = { ...titleItem.layout, motion: undefined };
    expect(withAnimation(layout, 'enter', 'slideLeft', 5_000_000)?.enter).toEqual({
      preset: 'klipolo.builtin/motion.slide',
      params: { from: 'right' },
      durationUs: 300_000,
    });
    expect(withEmphasis(layout, 'shake', 5_000_000)?.loop).toEqual(use('shake', 600_000, { strength: 1.5 }));
    expect(withEmphasis({ ...layout, motion: chatMotion }, 'none', 5_000_000)).toEqual({
      enter: chatMotion.enter,
      exit: chatMotion.exit,
    });
    // A loop no longer than the layer.
    expect(withEmphasis(layout, 'pulse', 500_000)?.loop?.durationUs).toBe(500_000);
  });

  it('shows what chat set and changes the loop as one step', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'In: Bounce' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Out: Slide right' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Loop: Pulse' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Wiggle' }));
    expect(apply).toHaveBeenCalledWith(
      'text.animation.undo',
      [
        {
          type: 'updateItem',
          id: 'heart',
          patch: {
            layout: { ...picture.layout, motion: { ...chatMotion, loop: use('wiggle', 800_000, { strength: 1.5 }) } },
          },
        },
      ],
      undefined,
      expect.objectContaining({ projectId: 'project' }),
    );
  });

  it('is on a video layer too', () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].items.push({
      ...picture,
      id: 'pip',
      content: { type: 'video', assetId: 'broll', sourceInUs: 0, speed: 1, volume: 1, muted: false },
    });
    act(() => {
      useProject.setState({ snapshot });
      useSelection.setState({ selectedId: 'pip' });
    });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'In: Bounce' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Loop: Pulse' })).toBeInTheDocument();
  });
});
