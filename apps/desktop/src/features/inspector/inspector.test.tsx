import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Op } from '../../bindings/Op';
import type { TextStyle } from '../../bindings/TextStyle';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The docked inspector card: its text, picture and sound fields each commit one labelled change, built at
 * its turn through the commit queue. A main-track clip's card is tested in `clipInspector.test.tsx`.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
// Rust's bundled families: the Font menu lists these once they are in.
vi.mock('../../bridge/fonts', () => ({
  fonts: {
    bundled: vi.fn(() =>
      Promise.resolve([
        { family: 'Inter', weight: 400, italic: false },
        { family: 'Inter', weight: 700, italic: false },
        { family: 'Anton', weight: 400, italic: false },
      ]),
    ),
    face: vi.fn(() => Promise.reject(new Error('no faces in tests'))),
  },
}));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');
const { BOX_COLOR } = await import('./inspectorOps');

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

afterEach(() => vi.useRealTimers());

type StylePatch = { patch: { content: { style: TextStyle } } };
/** Where a change through the commit queue was asked for (`useProject.apply`'s `built`): every edit goes in that way. */
const queued = expect.objectContaining({ projectId: 'project', revision: 3 });
const styleOf = (call: Parameters<typeof apply>) => (call[1][0] as unknown as StylePatch).patch.content.style;
type AudioContent = Extract<ItemContent, { type: 'audio' }>;
const contentOf = (call: Parameters<typeof apply>) =>
  (call[1][0] as unknown as { patch: { content: AudioContent } }).patch.content;
/** Each op's layer and the content it sets. */
const contents = (call: Parameters<typeof apply>) =>
  call[1].map((op) => {
    const { id, patch } = op as unknown as { id: string; patch: { content: AudioContent } };
    return [id, patch.content] as const;
  });

/**
 * The fixture's song laid as one bed of three pieces back to back, 0 to 30 s: music of one file at 30 %, ducking
 * 12 dB, fading in on the first piece and out on the last, as `add_music_ops` lays a short song under a longer video.
 */
function bed() {
  const snapshot = fixtureSnapshot();
  const audio = snapshot.project.timeline.tracks[1];
  const [song] = audio.items;
  const music: AudioContent = { ...(song.content as AudioContent), volume: 0.3, duck: -12 };
  audio.items = ['bed', 'bed-2', 'bed-3'].map((id, k) => ({
    ...song,
    id,
    startUs: k * 10_000_000,
    durationUs: 10_000_000,
    content: { ...music, fadeInUs: k === 0 ? 500_000 : 0, fadeOutUs: k === 2 ? 2_000_000 : 0 },
  }));
  return snapshot;
}

describe('the inspector', () => {
  it('is not there while nothing it has fields for is selected', () => {
    const { rerender } = renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('complementary')).toBeNull();
    // Something that's gone (removed since it was selected).
    act(() => useSelection.setState({ selectedId: 'gone' }));
    rerender(<Inspector format="landscape" />);
    expect(screen.queryByRole('complementary')).toBeNull();
  });

  it('shows a title’s fields and commits one colour change', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'Title settings' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Text' })).toHaveValue('Set up billing');
    expect(screen.getByRole('button', { name: 'Font: Inter' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Size' })).toHaveAttribute('aria-valuenow', '72');
    expect(screen.getByRole('radiogroup', { name: 'Background' })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'Top' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('button', { name: 'In: Fade' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Out: Fade' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Starts' })).toHaveValue('0:01.0');
    expect(screen.getByRole('textbox', { name: 'Lasts' })).toHaveValue('3.0 s');
    await userEvent.click(
      within(screen.getByRole('radiogroup', { name: 'Text color' })).getByRole('radio', { name: 'Yellow' }),
    );
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith(
      'text.color.undo',
      [
        expect.objectContaining({
          type: 'updateItem',
          id: 'title',
          patch: {
            content: expect.objectContaining({ style: expect.objectContaining({ color: '#FFD02E' }) }),
          },
        }),
      ],
      undefined,
      queued,
    );
  });

  it('the animation selects write motion presets and keep the rest of the motion', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'In: Fade' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Slide up' }));
    // It comes in from below.
    const slide = { preset: 'klipolo.builtin/motion.slide', params: { from: 'bottom' }, durationUs: 300_000 };
    expect(apply).toHaveBeenCalledWith(
      'text.animation.undo',
      [
        {
          type: 'updateItem',
          id: 'title',
          patch: { layout: { ...titleItem.layout, motion: { ...titleItem.layout.motion, enter: slide } } },
        },
      ],
      undefined,
      queued,
    );
    // "None" on both ends leaves no motion at all.
    const noEnter = {
      ...titleItem,
      layout: { ...titleItem.layout, motion: { exit: titleItem.layout.motion?.exit } },
    };
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].items = [noEnter];
    act(() => useProject.setState({ snapshot }));
    await userEvent.click(screen.getByRole('button', { name: 'Out: Fade' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'None' }));
    expect(apply).toHaveBeenLastCalledWith(
      'text.animation.undo',
      [{ type: 'updateItem', id: 'title', patch: { layout: { ...titleItem.layout, motion: undefined } } }],
      undefined,
      queued,
    );
  });

  it('an opacity drag previews in the engine and commits one change on release', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].items.push({
      ...titleItem,
      id: 'logo-layer',
      startUs: 5_000_000,
      attach: null,
      content: { type: 'image', assetId: 'logo' },
    });
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'logo-layer' });
    renderWithProviders(<Inspector format="landscape" />);
    const thumb = screen.getByRole('slider', { name: 'Opacity' });
    const root = thumb.parentElement!.parentElement!;
    // jsdom has no layout or pointer capture: a 100 px track, captured for the press.
    root.getBoundingClientRect = () => ({ left: 0, width: 100, top: 0, height: 20 }) as DOMRect;
    Object.assign(Element.prototype, {
      setPointerCapture: () => undefined,
      releasePointerCapture: () => undefined,
      hasPointerCapture: () => true,
    });
    fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: 60 });
    fireEvent.pointerMove(root, { pointerId: 1, clientX: 40 });
    expect(thumb).toHaveAttribute('aria-valuetext', '40%');
    await waitFor(() => expect(engine.preview).toHaveBeenCalled());
    expect(apply).not.toHaveBeenCalled();
    fireEvent.pointerUp(root, { pointerId: 1, clientX: 40 });
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('media.opacity.undo');
    expect(ops).toEqual([
      { type: 'updateItem', id: 'logo-layer', patch: { layout: expect.objectContaining({ opacity: 0.4 }) } },
    ]);
    expect(vi.mocked(engine.preview).mock.calls.at(-1)).toEqual(['project', 3, ops]);
  });

  it('sound layers get their own fields; locked layers offer Unlock', async () => {
    useSelection.setState({ selectedId: 'song' });
    const { unmount } = renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('slider', { name: 'Volume' })).toHaveAttribute('aria-valuetext', '40%');
    expect(screen.getByRole('radiogroup', { name: 'Fade in' })).toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Text' })).toBeNull();
    unmount();
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].locked = true;
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('button', { name: /^Font/ })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Unlock' }));
    expect(apply).toHaveBeenCalledWith(
      'timeline.track.unlock',
      [{ type: 'updateTrack', id: 't1', patch: { locked: false } }],
      undefined,
      queued,
    );
  });

  it('quick size keys and a custom colour typed in the colour popover are one change each', async () => {
    vi.useFakeTimers();
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    const size = screen.getByRole('slider', { name: 'Size' });
    for (let i = 0; i < 3; i++) fireEvent.keyDown(size, { key: 'ArrowRight' });
    expect(apply).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(400));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('text.size.undo');
    expect(styleOf(apply.mock.calls[0]).size).toBe(75);
    // It lands (the stand-in answers at once): a burst holds the commit queue until then, and the next waits.
    await act(async () => undefined);

    fireEvent.click(screen.getByRole('button', { name: /^Text color: more colors$/ }));
    const hex = screen.getByRole('textbox', { name: 'Hex color' });
    fireEvent.change(hex, { target: { value: '778899' } });
    fireEvent.keyDown(hex, { key: 'Enter' });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[1][0]).toBe('text.color.undo');
    expect(styleOf(apply.mock.calls[1]).color).toBe('#778899');
  });

  it('the background, position and timing are one labelled change each', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('radio', { name: 'Box' }));
    await userEvent.click(screen.getByRole('radio', { name: 'None' }));
    await userEvent.click(screen.getByRole('radio', { name: 'Bottom left' }));
    const starts = screen.getByRole('textbox', { name: 'Starts' });
    await userEvent.clear(starts);
    await userEvent.type(starts, '0:05.5{Enter}');
    const lasts = screen.getByRole('textbox', { name: 'Lasts' });
    await userEvent.clear(lasts);
    await userEvent.type(lasts, '2{Enter}');
    expect(apply.mock.calls.map(([label]) => label)).toEqual([
      'text.background.undo',
      'text.background.undo',
      'layout.position.undo',
      'timing.start.undo',
      'timing.length.undo',
    ]);
    expect(styleOf(apply.mock.calls[0])).toMatchObject({ background: BOX_COLOR, shadow: false });
    expect(styleOf(apply.mock.calls[1])).toMatchObject({ background: null, shadow: false });
    const placed = (apply.mock.calls[2][1][0] as unknown as { patch: { layout: { rect: { x: number; y: number } } } })
      .patch.layout;
    // 4 % of the 1080 px short side in from the left and the bottom of the 16:9 frame; the anchor follows.
    expect(placed.rect.x).toBeCloseTo((0.04 * 1080) / 1920);
    expect(placed.rect.y).toBeCloseTo(1 - 0.04 - 0.12);
    expect(placed).toMatchObject({ anchor: 'bottomLeft' });
    expect(apply.mock.calls[3][1]).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 5_500_000 }]);
    expect(apply.mock.calls[4][1]).toEqual([{ type: 'updateItem', id: 'title', patch: { durationUs: 2_000_000 } }]);
  });

  it('Escape in the text field puts the saved words back and commits nothing; the input method keeps Return', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    const field = screen.getByRole('textbox', { name: 'Text' });
    await userEvent.type(field, ' today');
    // Return that ends a composition (WebKit's keyCode 229) is the input method's: nothing is saved.
    fireEvent.keyDown(field, { key: 'Enter', keyCode: 229 });
    fireEvent.keyDown(field, { key: 'Enter', isComposing: true });
    expect(field).toHaveFocus();
    expect(apply).not.toHaveBeenCalled();
    fireEvent.keyDown(field, { key: 'Escape' });
    expect(field).toHaveValue('Set up billing');
    expect(field).not.toHaveFocus();
    await act(() => Promise.resolve());
    expect(apply).not.toHaveBeenCalled();
  });

  it('Escape in a title template’s words puts them back, in the field and the preview, and commits nothing', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].items.push({
      ...titleItem,
      id: 'card',
      startUs: 10_000_000,
      attach: null,
      content: {
        type: 'generator',
        source: 'klipolo.builtin/scene.lower-third',
        params: { title: 'Rosa Diaz', role: 'Potter' },
      },
    } as unknown as (typeof snapshot.project.timeline.tracks)[0]['items'][0]);
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'card' });
    renderWithProviders(<Inspector format="landscape" />);
    const name = screen.getByRole('textbox', { name: 'Name' });
    await userEvent.clear(name);
    await userEvent.type(name, 'Teo');
    expect(engine.preview).toHaveBeenCalled();
    fireEvent.keyDown(name, { key: 'Enter', isComposing: true });
    expect(name).toHaveValue('Teo');
    fireEvent.keyDown(name, { key: 'Escape' });
    expect(name).toHaveValue('Rosa Diaz');
    expect(engine.previewClear).toHaveBeenCalled();
    fireEvent.blur(name);
    await act(() => Promise.resolve());
    expect(apply).not.toHaveBeenCalled();
  });

  it('the Font menu lists Rust’s bundled families, and the size goes from 8 to 400', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    const size = screen.getByRole('slider', { name: 'Size' });
    expect(size).toHaveAttribute('aria-valuemin', '8');
    expect(size).toHaveAttribute('aria-valuemax', '400');
    await userEvent.click(screen.getByRole('button', { name: 'Font: Inter' }));
    await waitFor(() =>
      expect(screen.getAllByRole('menuitemradio').map((item) => item.textContent)).toEqual(['Inter', 'Anton']),
    );
  });

  it('a run of arrow keys on the colours or the position is one change, made once the keys stop', async () => {
    vi.useFakeTimers();
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Inspector format="landscape" />);
    const white = screen.getByRole('radio', { name: 'White' });
    act(() => white.focus());
    for (let i = 0; i < 3; i++) fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' });
    // The ring moves at once; nothing is committed yet.
    expect(screen.getByRole('radio', { name: 'Blue' })).toHaveAttribute('aria-checked', 'true');
    expect(apply).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(400));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('text.color.undo');
    expect(styleOf(apply.mock.calls[0]).color).toBe('#7F92DA');
    await act(async () => undefined);
    // The position grid: two presses, then focus leaves, and the run goes in then.
    const top = screen.getByRole('radio', { name: 'Top' });
    act(() => top.focus());
    fireEvent.keyDown(top, { key: 'ArrowDown' });
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' });
    expect(apply).toHaveBeenCalledTimes(1);
    act(() => screen.getByRole('slider', { name: 'Size' }).focus());
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[1][0]).toBe('layout.position.undo');
    expect((apply.mock.calls[1][1][0] as unknown as { patch: { layout: { anchor: string } } }).patch.layout.anchor).toBe(
      'middleRight',
    );
  });

  it('music’s ducking turns off and back on at the depth it had, never shallower than 1 dB; its fades are one change', async () => {
    const snapshot = fixtureSnapshot();
    const song = snapshot.project.timeline.tracks[1].items[0];
    song.content = { ...(song.content as AudioContent), duck: -18 };
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'song' });
    const { unmount } = renderWithProviders(<Inspector format="landscape" />);
    const depth = screen.getByRole('slider', { name: 'How much quieter' });
    expect(depth).toHaveAttribute('aria-valuemin', '1');
    expect(depth).toHaveAttribute('aria-valuenow', '18');
    await userEvent.click(screen.getByRole('radio', { name: 'Off' }));
    expect(apply.mock.calls[0][0]).toBe('music.duck.undo');
    expect(contentOf(apply.mock.calls[0]).duck).toBe(0);
    await userEvent.click(within(screen.getByRole('radiogroup', { name: 'Fade in' })).getByRole('radio', { name: '0.5 s' }));
    expect(apply.mock.calls[1][0]).toBe('clip.fades.undo');
    unmount();
    // Off now: back on comes back at 18 dB, not the default 12.
    const off = fixtureSnapshot();
    const quiet = off.project.timeline.tracks[1].items[0];
    quiet.content = { ...(quiet.content as AudioContent), duck: 0 };
    useProject.setState({ snapshot: off });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('slider', { name: 'How much quieter' })).toBeNull();
    await userEvent.click(screen.getByRole('radio', { name: 'On' }));
    expect(contentOf(apply.mock.calls[2]).duck).toBe(-18);
  });

  it('a song laid in pieces is one: its volume and ducking go on every piece, its fades on its first and last', async () => {
    useProject.setState({ snapshot: bed() });
    useSelection.setState({ selectedId: 'bed-2' });
    renderWithProviders(<Inspector format="landscape" />);
    // The volume, set on the middle piece: all three at 60 %, one change.
    const thumb = screen.getByRole('slider', { name: 'Volume' });
    const root = thumb.parentElement!.parentElement!;
    root.getBoundingClientRect = () => ({ left: 0, width: 100, top: 0, height: 20 }) as DOMRect;
    Object.assign(Element.prototype, {
      setPointerCapture: () => undefined,
      releasePointerCapture: () => undefined,
      hasPointerCapture: () => true,
    });
    fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: 30 });
    fireEvent.pointerUp(root, { pointerId: 1, clientX: 30 });
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('volume.undo');
    expect(contents(apply.mock.calls[0]).map(([id, c]) => [id, c.volume])).toEqual([
      ['bed', 0.6],
      ['bed-2', 0.6],
      ['bed-3', 0.6],
    ]);
    // Ducking off: every piece, one change; each keeps its own fades.
    await userEvent.click(screen.getByRole('radio', { name: 'Off' }));
    expect(apply.mock.calls[1][0]).toBe('music.duck.undo');
    expect(contents(apply.mock.calls[1]).map(([id, c]) => [id, c.duck, c.fadeInUs, c.fadeOutUs])).toEqual([
      ['bed', 0, 500_000, 0],
      ['bed-2', 0, 0, 0],
      ['bed-3', 0, 0, 2_000_000],
    ]);
    // The fades are the song's: in on its first piece, out on its last.
    const fadeIn = screen.getByRole('radiogroup', { name: 'Fade in' });
    const fadeOut = screen.getByRole('radiogroup', { name: 'Fade out' });
    expect(within(fadeIn).getByRole('radio', { name: '0.5 s' })).toHaveAttribute('aria-checked', 'true');
    expect(within(fadeOut).getByRole('radio', { name: '2 s' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(within(fadeIn).getByRole('radio', { name: '1 s' }));
    expect(contents(apply.mock.calls[2]).map(([id, c]) => [id, c.fadeInUs])).toEqual([['bed', 1_000_000]]);
    await userEvent.click(within(fadeOut).getByRole('radio', { name: '1 s' }));
    expect(contents(apply.mock.calls[3]).map(([id, c]) => [id, c.fadeOutUs])).toEqual([['bed-3', 1_000_000]]);
  });
});
