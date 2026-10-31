import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import { useAi } from '../../ai/aiStatus';
import { useAnalysis } from '../../state/analysis';
import { useChat } from '../../state/chat';
import { useProject } from '../../state/project';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { MessageList } from './MessageList';

/** The files the fixture project's main-track clips play (`fixtureProject`): its song is a layer, not analysed. */
const CLIPS = ['screen', 'camera', 'mic', 'broll'];
let estimated: unknown[] = [];

beforeEach(() => {
  estimated = [];
  mockIPC((cmd, args) => {
    if (cmd !== 'analysis_estimate') return null;
    estimated.push(args);
    return { usd: 0.04, seconds: 130, cached: [], clips: CLIPS, missing: [] };
  });
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useAi.setState({ status: { mode: 'key', keyHint: '6789' } });
  useChat.getState().reset(fixtureSnapshot().project.id);
  useAnalysis.setState({ states: {}, estimate: null, error: null, dismissed: [], busy: false });
});

describe('the empty chat’s analysis note', () => {
  it('offers Analyze while the clips aren’t analysed, even after Not now', async () => {
    useAnalysis.setState({ dismissed: CLIPS });
    renderWithProviders(<MessageList />);

    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));

    expect(estimated).toEqual([{ assetIds: CLIPS }]);
    expect(useAnalysis.getState().dismissed).toEqual([]);
    expect(useAnalysis.getState().estimate?.clips).toEqual(CLIPS);
  });

  it('has no Analyze when every clip not done failed: the banner’s Retry is the way on', () => {
    const done = { state: 'done', approximate: false } as const;
    const failed = { state: 'failed', reason: 'Klipolo could not read this clip.' } as const;
    useAnalysis.setState({ states: { screen: failed, camera: done, mic: done, music: done, broll: done } });
    renderWithProviders(<MessageList />);
    expect(screen.getByText(/Analyze the clips first/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Analyze' })).toBeNull();
  });

  it('has no Analyze while the clips are being analysed', () => {
    useAnalysis.setState({ states: { screen: { state: 'running', step: 'words', pct: 40 } } });
    renderWithProviders(<MessageList />);
    expect(screen.getByText(/still listening/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Analyze' })).toBeNull();
  });
});
