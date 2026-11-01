import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { Keep } from '../../bindings/Keep';
import type { Person } from '../../bindings/Person';
import type { ClipFraming } from '../../bindings/ClipFraming';
import type { Op } from '../../bindings/Op';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import type { SpeakerEstimate } from '../../bindings/SpeakerEstimate';
import type { SpeakerProgress } from '../../bindings/SpeakerProgress';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The clip card's Focus while Fill crops, per format: Speaker (an imported video or a camera take, its price said
 * first, its progress while Rust tracks), Fixed (the Fill focus slider) or Centre. Each is one undo step. A clip that
 * follows two or more people also says who it keeps: whoever is speaking, one of them by position, or both; chat's own
 * choices show as such, with a reset.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
const adopt = vi.fn((_change: SnapshotPatch) => Promise.resolve(true));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const fill = { mode: 'fill', focusX: 0.5, focusY: 0.5 } as const;
const WIDE = {
  landscape: { x: 0, y: 0, w: 1, h: 1 },
  portrait: { x: 0.341796875, y: 0, w: 0.31640625, h: 1 },
  square: { x: 0.21875, y: 0, w: 0.5625, h: 1 },
};
/** Clip b (broll.mp4) is an imported 16:9 video: it can follow its speaker, filling from the centre everywhere. */
const FRAMING: Record<string, ClipFraming> = {
  b: { framing: { landscape: fill, portrait: fill, square: fill }, follows: false, speaker: true, fillCrop: WIDE },
};
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn((clipId: string) => Promise.resolve(FRAMING[clipId])),
}));
let progress: ((p: SpeakerProgress) => void) | null = null;
vi.mock('../../bridge/speaker', () => ({
  speakerBridge: {
    estimate: vi.fn(() => Promise.resolve({ usd: 0.024, clips: ['b'], cached: false } satisfies SpeakerEstimate)),
    follow: vi.fn(() => Promise.resolve({} as SnapshotPatch)),
    keep: vi.fn(() => Promise.resolve({ baseRevision: 3 } as SnapshotPatch)),
    onProgress: vi.fn((handler: (p: SpeakerProgress) => void) => {
      progress = handler;
      return Promise.resolve(() => {
        progress = null;
      });
    }),
  },
}));
const { speakerBridge } = await import('../../bridge/speaker');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

const queued = expect.objectContaining({ projectId: 'project', revision: 3 });

function withB(patch: Partial<Clip>) {
  const snapshot = fixtureSnapshot();
  const main = snapshot.project.timeline.main;
  main[1] = { ...main[1], ...patch };
  return snapshot;
}

beforeEach(() => {
  vi.clearAllMocks();
  progress = null;
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply, adopt });
  useSelection.setState({ selectedId: 'b' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('the Focus of a filled clip', () => {
  it('offers Speaker, Fixed and Centre where Fill crops, Fixed by default, with the price said first', async () => {
    renderWithProviders(<Inspector format="portrait" />);
    const fixed = await screen.findByRole('radio', { name: 'Fixed' });
    expect(fixed).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('radio', { name: 'Speaker' })).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByRole('radio', { name: 'Centre' })).toBeInTheDocument();
    // An old project's clip has no focus of its own: Fixed, its Fill focus slider below.
    expect(screen.getByRole('slider', { name: 'Focus across' })).toBeInTheDocument();
    expect(await screen.findByText('Follow the speaker, about $0.02')).toBeInTheDocument();
    expect(speakerBridge.estimate).toHaveBeenCalledWith('b', 'portrait');
    expect(speakerBridge.follow).not.toHaveBeenCalled();
  });

  it('is not offered where Fill crops nothing', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await screen.findByRole('radio', { name: 'Fill', checked: true });
    expect(screen.queryByRole('radio', { name: 'Speaker' })).toBeNull();
    expect(screen.queryByRole('radio', { name: 'Centre' })).toBeNull();
  });

  it('Centre and Fixed are one step each in the format shown, the other formats kept', async () => {
    useProject.setState({ snapshot: withB({ fillFocus: { square: { kind: 'center' } } }) });
    const user = userEvent.setup();
    const { unmount } = renderWithProviders(<Inspector format="portrait" />);
    await user.click(await screen.findByRole('radio', { name: 'Centre' }));
    expect(apply).toHaveBeenCalledWith(
      'clip.focus.center.undo',
      [
        {
          type: 'updateClip',
          id: 'b',
          patch: { fillFocus: { square: { kind: 'center' }, portrait: { kind: 'center' } } },
        },
      ],
      undefined,
      queued,
    );
    unmount();
    // In square it is Centre already: no slider; Fixed takes its own focus away again.
    renderWithProviders(<Inspector format="square" />);
    expect(await screen.findByRole('radio', { name: 'Centre', checked: true })).toBeInTheDocument();
    expect(screen.queryByRole('slider', { name: 'Focus across' })).toBeNull();
    await user.click(screen.getByRole('radio', { name: 'Fixed' }));
    expect(apply).toHaveBeenLastCalledWith(
      'clip.focus.fixed.undo',
      [{ type: 'updateClip', id: 'b', patch: { fillFocus: {} } }],
      undefined,
      queued,
    );
  });

  it('Speaker tracks with its progress shown, then takes the change Rust made', async () => {
    let finish!: (change: SnapshotPatch) => void;
    const change = { baseRevision: 3 } as SnapshotPatch;
    vi.mocked(speakerBridge.follow).mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    const user = userEvent.setup();
    renderWithProviders(<Inspector format="portrait" />);
    await screen.findByText('Follow the speaker, about $0.02');
    await user.click(screen.getByRole('radio', { name: 'Speaker' }));
    expect(speakerBridge.follow).toHaveBeenCalledWith('b', 'portrait');
    expect(await screen.findByRole('status')).toHaveTextContent('Finding the speaker, 0%');
    await act(async () => progress?.({ clipIds: ['b'], pct: 40 }));
    expect(screen.getByRole('status')).toHaveTextContent('Finding the speaker, 40%');
    // Another click while it runs does nothing more.
    expect(screen.getByRole('radio', { name: 'Speaker' })).toBeDisabled();
    await act(async () => finish(change));
    await waitFor(() => expect(adopt).toHaveBeenCalledWith(change));
    expect(screen.queryByRole('status')).toBeNull();
    expect(speakerBridge.follow).toHaveBeenCalledTimes(1);
    expect(apply).not.toHaveBeenCalled();
  });

  it('a clip that follows its speaker shows Speaker, and nothing to pay', async () => {
    const path = { keys: [{ atUs: 0, x: 0.3, y: 0.5 }] };
    useProject.setState({
      snapshot: withB({ framing: { portrait: fill }, fillFocus: { portrait: { kind: 'speaker', path } } }),
    });
    renderWithProviders(<Inspector format="portrait" />);
    expect(await screen.findByRole('radio', { name: 'Speaker', checked: true })).toBeInTheDocument();
    expect(screen.queryByRole('slider', { name: 'Focus across' })).toBeNull();
    expect(screen.queryByText(/Follow the speaker,/)).toBeNull();
  });

  it('a refusal is said instead of the price, and Speaker is off and sends nothing', async () => {
    vi.mocked(speakerBridge.estimate).mockResolvedValue({
      usd: 0.024,
      clips: ['b'],
      cached: false,
      refusal: 'Add your Meta API key or access code in Settings.',
    });
    const user = userEvent.setup();
    renderWithProviders(<Inspector format="portrait" />);
    const note = await screen.findByText('Add your Meta API key or access code in Settings.');
    const speaker = screen.getByRole('radio', { name: 'Speaker' });
    expect(speaker).toBeDisabled();
    expect(speaker).toHaveAttribute('aria-describedby', note.id);
    await user.click(speaker);
    expect(speakerBridge.follow).not.toHaveBeenCalled();
  });

  it('Speaker stays off until its price has come, so nothing is paid for unseen', async () => {
    vi.mocked(speakerBridge.estimate).mockReturnValueOnce(new Promise(() => undefined));
    const user = userEvent.setup();
    renderWithProviders(<Inspector format="portrait" />);
    const speaker = await screen.findByRole('radio', { name: 'Speaker' });
    expect(speaker).toBeDisabled();
    await user.click(speaker);
    expect(speakerBridge.follow).not.toHaveBeenCalled();
  });

  it('after an edit the old price is gone, and Speaker is off until the new one comes', async () => {
    vi.mocked(speakerBridge.estimate).mockResolvedValue({ usd: 0.024, clips: ['b'], cached: false });
    renderWithProviders(<Inspector format="portrait" />);
    expect(await screen.findByText('Follow the speaker, about $0.02')).toBeInTheDocument();
    vi.mocked(speakerBridge.estimate).mockReturnValueOnce(new Promise(() => undefined));
    const snapshot = useProject.getState().snapshot!;
    act(() =>
      useProject.setState({
        snapshot: { ...snapshot, history: { ...snapshot.history, revision: snapshot.history.revision + 1 } },
      }),
    );
    await waitFor(() => expect(screen.queryByText('Follow the speaker, about $0.02')).toBeNull());
    expect(screen.getByRole('radio', { name: 'Speaker' })).toBeDisabled();
  });

  it('footage tracked before is followed at no charge', async () => {
    vi.mocked(speakerBridge.estimate).mockResolvedValue({ usd: 0, clips: ['b'], cached: true });
    renderWithProviders(<Inspector format="square" />);
    expect(await screen.findByText('Follow the speaker, no charge')).toBeInTheDocument();
  });

  describe('who a clip following two or more people keeps', () => {
    const path = { keys: [{ atUs: 0, x: 0.3, y: 0.5 }] };
    const two = [
      { voices: [{ part: 0, label: 'A' }], share: 60 },
      { voices: [{ part: 0, label: 'B' }], share: 40 },
    ];
    const following = (keep?: Keep, people: Person[] = two) =>
      withB({ framing: { portrait: fill }, fillFocus: { portrait: { kind: 'speaker', path, keep, people } } });

    it('keeps whoever is speaking, and offers each by position and both, each one step', async () => {
      useProject.setState({ snapshot: following() });
      const user = userEvent.setup();
      renderWithProviders(<Inspector format="portrait" />);
      const keep = await screen.findByRole('radiogroup', { name: 'Keep' });
      const names = within(keep)
        .getAllByRole('radio')
        .map((r) => r.textContent);
      expect(names).toEqual(['Speaking', 'Left', 'Right', 'Both']);
      expect(within(keep).getByRole('radio', { name: 'Speaking' })).toHaveAttribute('aria-checked', 'true');
      await user.click(within(keep).getByRole('radio', { name: 'Right' }));
      expect(speakerBridge.keep).toHaveBeenCalledWith('b', 'portrait', { kind: 'person', person: 2 });
      await waitFor(() => expect(adopt).toHaveBeenCalledWith({ baseRevision: 3 }));
      expect(apply).not.toHaveBeenCalled();
    });

    it('shows one person or both as chosen', async () => {
      useProject.setState({ snapshot: following({ kind: 'person', person: 1 }) });
      const { unmount } = renderWithProviders(<Inspector format="portrait" />);
      const keep = await screen.findByRole('radiogroup', { name: 'Keep' });
      expect(within(keep).getByRole('radio', { name: 'Left' })).toHaveAttribute('aria-checked', 'true');
      unmount();
      useProject.setState({ snapshot: following({ kind: 'both' }) });
      renderWithProviders(<Inspector format="portrait" />);
      const again = await screen.findByRole('radiogroup', { name: 'Keep' });
      expect(within(again).getByRole('radio', { name: 'Both' })).toHaveAttribute('aria-checked', 'true');
    });

    it("says chat's own choices as custom by chat, with a reset to whoever is speaking", async () => {
      const ranges = [{ startUs: 2_000_000, endUs: 8_000_000, who: { kind: 'person' as const, person: 2 } }];
      useProject.setState({ snapshot: following({ kind: 'ranges', ranges }) });
      const user = userEvent.setup();
      renderWithProviders(<Inspector format="portrait" />);
      expect(await screen.findByText('Custom by chat')).toBeInTheDocument();
      const keep = screen.getByRole('radiogroup', { name: 'Keep' });
      expect(within(keep).queryByRole('radio', { checked: true })).toBeNull();
      await user.click(screen.getByRole('button', { name: 'Reset' }));
      expect(speakerBridge.keep).toHaveBeenCalledWith('b', 'portrait', { kind: 'speaker' });
    });

    it('names three people left, middle and right, and is not shown for one', async () => {
      useProject.setState({ snapshot: following(undefined, [...two, { share: 0 }]) });
      const { unmount } = renderWithProviders(<Inspector format="portrait" />);
      const keep = await screen.findByRole('radiogroup', { name: 'Keep' });
      expect(within(keep).getAllByRole('radio').map((r) => r.textContent)).toEqual([
        'Speaking',
        'Left',
        'Middle',
        'Right',
        'All',
      ]);
      unmount();
      useProject.setState({ snapshot: following(undefined, []) });
      renderWithProviders(<Inspector format="portrait" />);
      await screen.findByRole('radio', { name: 'Speaker', checked: true });
      expect(screen.queryByRole('radiogroup', { name: 'Keep' })).toBeNull();
    });
  });
});
