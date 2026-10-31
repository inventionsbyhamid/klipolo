import { expect, it } from 'vitest';
import type { WordView } from '../../bindings/WordView';
import { useProject } from '../../state/project';
import { fixtureSnapshot } from '../../test/fixtures';
import { currentLook } from './captionOps';

it('reads a split correction’s look where the pages read it, at the piece it shows', () => {
  // Chat coloured the piece of a correction a split left on the page (1 to 1.5 s of the mic); the word's range is the
  // whole correction's (1 to 4 s), whose middle lies outside that look.
  const snap = fixtureSnapshot();
  snap.project.wordLooks = [
    { assetId: 'mic', startUs: 1_000_000, endUs: 1_500_000, look: { color: '#32D74B' } },
  ];
  useProject.setState({ snapshot: snap });
  const fixed: WordView = {
    text: 'Fixed three words',
    joined: false,
    startUs: 0,
    endUs: 500_000,
    assetId: 'mic',
    sourceStartUs: 1_000_000,
    sourceEndUs: 4_000_000,
    look: {},
    keyword: null,
  };
  expect(currentLook({ ...fixed, lookUs: 1_250_000 })).toEqual({ color: '#32D74B' });
  // A word the pages read at its middle.
  expect(currentLook({ ...fixed, sourceEndUs: 1_400_000 })).toEqual({ color: '#32D74B' });
});
