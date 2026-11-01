import { act, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Project } from '../../bindings/Project';
import { fixtureProject, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { audioRole } = await import('./lanes/audioRole');
const { dropRefusal, dropTargetAt, setDropIndicator, useDropIndicator } = await import('./dropTarget');
const { layerStartUs } = await import('./model/dropOps');
const { usToPx } = await import('./scale');
const { videoDurationUs } = await import('../../lib/timeline');

const S = 1_000_000;
type AudioContent = Extract<ItemContent, { type: 'audio' }>;

/** The fixture with its song turned into music (ducking set) and a sound effect at 31 s on the same track. */
function withMusicAndSound(): Project {
  const project = fixtureProject();
  const audio = project.timeline.tracks.find((t) => t.kind === 'audio')!;
  const [song] = audio.items;
  const music: Item = { ...song, content: { ...(song.content as AudioContent), duck: -12 } };
  const whoosh: Item = { ...song, id: 'whoosh', startUs: 31 * S, durationUs: S };
  audio.items = [music, whoosh];
  return project;
}

beforeEach(() => {
  useSelection.setState({ selectedId: null });
  useUi.setState({ pxPerSecond: 40, snapping: true });
});

describe('the four lanes', () => {
  it('lanes_split_music_and_sounds', () => {
    const project = withMusicAndSound();
    useProject.setState({ snapshot: { ...fixtureSnapshot(), project }, pending: 0 });
    renderWithProviders(<Timeline />);
    // Four lanes, top to bottom, each with its icon's name; under Video, the take's recorded sounds (its mic, and its
    // screen file's own sound) have lanes of their own.
    expect(screen.getAllByRole('group').map((g) => g.getAttribute('aria-label'))).toEqual([
      'Overlays',
      'Video',
      'Mic',
      "Screen's sound",
      'Sound effects',
      'Music',
    ]);
    // The ducking song is music; the plain sound on the same track is a sound effect.
    const [song, whoosh] = project.timeline.tracks[1].items;
    expect(audioRole(song)).toBe('music');
    expect(audioRole(whoosh)).toBe('sounds');
    const music = screen.getByRole('group', { name: 'Music' });
    const sounds = screen.getByRole('group', { name: 'Sound effects' });
    expect(music.querySelector('[data-block-id="song"]')).not.toBeNull();
    expect(music.querySelector('[data-block-id="whoosh"]')).toBeNull();
    expect(sounds.querySelector('[data-block-id="whoosh"]')).toHaveAttribute('data-look', 'sound');
    // A lane with something on it is no faint strip.
    expect(sounds.querySelector('.tl-lane')).not.toHaveClass('is-empty');
  });

  it('a sound held over the Music lane shows on the Music lane, where it lands as music; each lane marks only its own', () => {
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    renderWithProviders(<Timeline />);
    const lane = (name: string) => screen.getByRole('group', { name }).querySelector<HTMLElement>('.tl-lane')!;
    // The lanes where a browser would put them, under a 20 px ruler (jsdom lays nothing out).
    const scroller = screen.getByTestId('ruler').closest<HTMLElement>('.tl-scroll')!;
    const box = (el: Element, top: number, height: number) =>
      (el.getBoundingClientRect = () => DOMRect.fromRect({ x: 0, y: top, width: 1240, height }));
    box(scroller, 0, 200);
    const rows = [
      ['layers', 20, 24],
      ['main', 49, 64],
      ['sounds', 118, 22],
      ['music', 145, 34],
    ] as const;
    for (const [name, top, height] of rows) box(document.querySelector(`[data-lane="${name}"]`)!, top, height);
    act(() => setDropIndicator(dropTargetAt(40 + 120, 160, 'audio')));
    expect(lane('Music')).toHaveClass('is-drop');
    expect(lane('Sound effects')).not.toHaveClass('is-drop');
    // Music fills the video from where it is let go: its ghost runs to the video's end, not the file's length.
    const ghost = document.querySelector<HTMLElement>('[data-drop-indicator]')!;
    const project = useProject.getState().snapshot!.project;
    const held = useDropIndicator.getState().target as { atUs: number };
    const from = layerStartUs(project, held.atUs, project.settings.fps);
    const pps = useUi.getState().pxPerSecond;
    expect(ghost.style.width).toBe(`${Math.max(4, usToPx(videoDurationUs(project.timeline) - from, pps))}px`);
    act(() => setDropIndicator(dropTargetAt(40 + 120, 125, 'audio')));
    expect(lane('Sound effects')).toHaveClass('is-drop');
    // A sound effect's ghost is its own length (a file from Finder: not known yet).
    expect(ghost.style.width).toBe('48px');
    expect(lane('Music')).not.toHaveClass('is-drop');
    // A picture over the Overlays lane marks that lane alone.
    act(() => setDropIndicator(dropTargetAt(40 + 120, 30, 'image')));
    expect(lane('Overlays')).toHaveClass('is-drop');
    expect(lane('Sound effects')).not.toHaveClass('is-drop');
    act(() => setDropIndicator(null));
    // Let go on the wrong lane, a picture or a video names the lane by the name its icon has.
    expect(dropRefusal(40 + 120, 125, 'image')).toBe('Pictures go on the Overlays lane.');
    expect(dropRefusal(40 + 120, 125, 'video')).toBe('Videos go on the Video or Overlays lane.');
  });
});
