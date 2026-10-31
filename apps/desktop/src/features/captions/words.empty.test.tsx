import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import { activateLocale } from '../../i18n';
import { useAnalysis } from '../../state/analysis';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { CaptionWords } from './CaptionWords';
import { forgetCaptionPages } from './useCaptionPages';

/** The files the fixture project's main-track clips play (`fixtureProject`): its song is a layer, not analysed. */
const CLIPS = ['screen', 'camera', 'mic', 'broll'];
let estimated: unknown[] = [];

beforeEach(async () => {
  await activateLocale('en');
  forgetCaptionPages();
  estimated = [];
  const snap = fixtureSnapshot();
  snap.project.captions = { ...snap.project.captions, enabled: true };
  useProject.setState({ snapshot: snap });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useAnalysis.setState({ states: {}, estimate: null, error: null, dismissed: [], busy: false });
  mockIPC((cmd, args) => {
    if (cmd === 'captions_pages')
      return { revision: 0, format: 'portrait', total: 0, first: 0, capped: false, pages: [] };
    if (cmd === 'analysis_estimate') {
      estimated.push(args);
      return { usd: 0.04, seconds: 130, cached: [], clips: CLIPS, missing: [] };
    }
    return null;
  });
});

const done = { state: 'done', approximate: false } as const;
const silent = { state: 'done', approximate: false, emptyReason: 'No speech found.' } as const;

describe('caption words with none to show', () => {
  it('says a clip heard with no speech has none', async () => {
    useAnalysis.setState({
      states: { screen: done, camera: done, mic: silent, music: done, broll: done },
    });
    renderWithProviders(<CaptionWords format="portrait" />);
    expect(await screen.findByText('No speech found in this clip.')).toBeInTheDocument();
    expect(screen.queryByText(/No caption words yet/)).toBeNull();
  });

  it('says so of several clips heard with no speech', async () => {
    useAnalysis.setState({
      states: { screen: silent, camera: done, mic: silent, music: done, broll: done },
    });
    renderWithProviders(<CaptionWords format="portrait" />);
    expect(await screen.findByText('No speech found in these clips.')).toBeInTheDocument();
  });

  it('offers Analyze while clips are not analysed, even after Not now', async () => {
    useAnalysis.setState({ dismissed: CLIPS });
    renderWithProviders(<CaptionWords format="portrait" />);
    expect(
      await screen.findByText('No caption words yet. They show here once Klipolo has heard the clips.'),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));
    expect(estimated).toEqual([{ assetIds: CLIPS }]);
    expect(useAnalysis.getState().dismissed).toEqual([]);
    expect(useAnalysis.getState().estimate?.clips).toEqual(CLIPS);
  });

  it('offers nothing while clips are being analysed', async () => {
    useAnalysis.setState({
      states: { screen: { state: 'running', step: 'transcript', pct: 30 } },
    });
    renderWithProviders(<CaptionWords format="portrait" />);
    expect(await screen.findByText(/No caption words yet/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Analyze' })).toBeNull();
  });
});
