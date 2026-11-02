import { act, render, screen } from '@testing-library/react';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from './test/fixtures';

/** Renders of each of the editor's panels (stand-ins here: what matters is whether the editor draws them again). */
const renders = vi.hoisted(() => ({ library: 0, preview: 0, timeline: 0, chat: 0 }));
vi.mock('./layout/Library', () => ({
  Library: () => {
    renders.library += 1;
    return <div data-testid="library" />;
  },
}));
vi.mock('./preview/Preview', () => ({
  Preview: () => {
    renders.preview += 1;
    return null;
  },
}));
vi.mock('./features/timeline/Timeline', () => ({
  Timeline: () => {
    renders.timeline += 1;
    return null;
  },
}));
vi.mock('./layout/Chat', () => ({
  Chat: () => {
    renders.chat += 1;
    return null;
  },
}));

const { App } = await import('./App');
const { useProject } = await import('./state/project');
const { useSelection } = await import('./state/selection');
const { usePlayback } = await import('./state/playback');

const snap = fixtureSnapshot();

beforeEach(() => {
  mockIPC((cmd) => {
    switch (cmd) {
      case 'project_snapshot':
        return snap;
      case 'projects_recent':
      case 'export_jobs':
      case 'analysis_states':
      case 'fonts_downloaded':
      case 'library_pack':
      case 'media_speech_ranges':
        return [];
      case 'fonts_missing':
        return { type: 'ready', fonts: [] };
      case 'library_marks':
        return {};
      case 'media_jobs_list':
        return { jobs: [], held: [] };
      case 'engine_pause':
        return 0;
      case 'engine_attach':
        return { port: 0, token: '', maxPreviewPixels: 0, adapter: '', presenter: 'native', gpuClass: 'integrated' };
      default:
        return null;
    }
  });
});

describe('the editor', () => {
  it("draws its panels again for none of the editor keys' changes: a selection, playing, an edit", async () => {
    render(<App />);
    await screen.findByTestId('library');
    await act(() => new Promise((r) => setTimeout(r, 50)));
    const before = { ...renders };
    act(() => useSelection.getState().select('title'));
    act(() => usePlayback.setState({ playing: true }));
    act(() => usePlayback.setState({ playing: false }));
    act(() => {
      const s = useProject.getState().snapshot!;
      useProject.setState({ snapshot: { ...s, history: { ...s.history, canRedo: true } } });
    });
    expect(renders).toEqual(before);
  });
});
