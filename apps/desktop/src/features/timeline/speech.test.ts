import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ItemContent } from '../../bindings/ItemContent';
import { fixtureSnapshot } from '../../test/fixtures';

/*
 * Where the music dips (`useSpeech`): asked of Rust while something ducks, again on every change of the project, and
 * again as a clip's analysis finishes, since that is when Rust has what was said in it.
 */

vi.mock('../../bridge/library', () => ({ library: { speechRanges: vi.fn() } }));
const { library } = await import('../../bridge/library');
const { useProject } = await import('../../state/project');
const { useAnalysis } = await import('../../state/analysis');
const { useSpeech, followSpeech } = await import('./media/speech');

type AudioContent = Extract<ItemContent, { type: 'audio' }>;

/** The fixture with its song as music that ducks 12 dB. */
function ducking() {
  const snapshot = fixtureSnapshot();
  const [song] = snapshot.project.timeline.tracks[1].items;
  song.content = { ...(song.content as AudioContent), duck: -12 };
  return snapshot;
}

beforeEach(() => {
  vi.mocked(library.speechRanges).mockReset().mockResolvedValue([]);
});

describe('the music’s dips under speech', () => {
  it('are asked for again as a clip’s analysis finishes', async () => {
    useProject.setState({ snapshot: ducking(), pending: 0 });
    useAnalysis.setState({ states: { a: { state: 'running', step: 'transcribe', pct: 40 } } as never });
    followSpeech();
    await vi.waitFor(() => expect(library.speechRanges).toHaveBeenCalledTimes(1));
    vi.mocked(library.speechRanges).mockResolvedValue([{ start: 5_000_000, end: 8_000_000 }]);
    // Progress alone asks nothing.
    useAnalysis.setState({ states: { a: { state: 'running', step: 'transcribe', pct: 80 } } as never });
    expect(library.speechRanges).toHaveBeenCalledTimes(1);
    useAnalysis.setState({ states: { a: { state: 'done', approximate: false } } as never });
    await vi.waitFor(() => expect(useSpeech.getState().ranges).toEqual([{ start: 5_000_000, end: 8_000_000 }]));
    expect(library.speechRanges).toHaveBeenCalledTimes(2);
  });
});
