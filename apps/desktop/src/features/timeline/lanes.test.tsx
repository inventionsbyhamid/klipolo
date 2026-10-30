import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
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

const S = 1_000_000;

/** The fixture with its song turned into music (ducking set) and a sound effect at 31 s on the same track. */
function withMusicAndSound(): Project {
  const project = fixtureProject();
  const audio = project.timeline.tracks.find((t) => t.kind === 'audio')!;
  const [song] = audio.items;
  const music = { ...song, content: { ...song.content, duck: -12 } } as unknown as Item;
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
    // Four lanes, top to bottom, each with its icon's name.
    expect(screen.getAllByRole('group').map((g) => g.getAttribute('aria-label'))).toEqual([
      'Text and images',
      'Video',
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
});
