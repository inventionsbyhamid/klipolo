import { act, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectRow } from '../../bindings/ProjectRow';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/* The project menu's Open recent items are commands too: the same projects, in the same order, opened the same way. */

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), openProject: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
vi.mock('../../bridge/projects', () => ({ projects: { recent: vi.fn(), reveal: vi.fn() } }));
const { bridge } = await import('../../bridge/commands');
const { projects } = await import('../../bridge/projects');
const { useProject } = await import('../../state/project');
const { readRecent, useRecentProjects } = await import('../../state/recentProjects');
const { useCommands } = await import('./useCommands');

const recentRow = (name: string, open = false): ProjectRow => ({
  path: `/Users/me/Movies/Klipolo/${name}.klipolo`,
  name,
  openedMs: 0,
  durationUs: 0,
  clips: 1,
  thumb: null,
  open,
});

/** The commands as the editor has them now. */
let commands: ReturnType<typeof useCommands> = [];
function Commands() {
  commands = useCommands();
  return null;
}
const openRecent = () => commands.filter((c) => c.id.startsWith('open-recent-'));

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, switching: false });
  useRecentProjects.setState({ rows: [], readFor: null, readAt: 0 });
  vi.mocked(bridge.openProject).mockReset();
  vi.mocked(projects.recent).mockReset();
});

describe('Open recent commands', () => {
  it('open_recent_projects_are_commands_that_open_them', async () => {
    vi.mocked(projects.recent).mockResolvedValue([
      recentRow('Billing tutorial', true),
      recentRow('Kiln vlog'),
      recentRow('Banner'),
    ]);
    const kiln: Snapshot = fixtureSnapshot();
    kiln.project = { ...kiln.project, id: 'kiln', name: 'Kiln vlog' };
    vi.mocked(bridge.openProject).mockResolvedValue({ ...kiln, opening: 2 });
    renderWithProviders(<Commands />);
    // None until the list has been read for the project open now.
    expect(openRecent()).toEqual([]);

    await act(() => readRecent());
    expect(openRecent().map((c) => [c.id, c.label, c.group, c.enabled])).toEqual([
      ['open-recent-1', 'Open Kiln vlog', 'project', true],
      ['open-recent-2', 'Open Banner', 'project', true],
    ]);
    expect(openRecent()[0].keywords).toEqual(['open recent', 'switch project']);

    act(() => openRecent()[0].run());
    // Once the open project's edits have landed, as the menu's open.
    await waitFor(() =>
      expect(bridge.openProject).toHaveBeenCalledWith('/Users/me/Movies/Klipolo/Kiln vlog.klipolo'),
    );
    await waitFor(() => expect(useProject.getState().snapshot?.project.id).toBe('kiln'));
    // The list was read for the project before: another project, no commands until it is read again.
    expect(openRecent()).toEqual([]);
  });
});
