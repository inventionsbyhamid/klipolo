import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SaveStatus } from '../bridge/commands';
import { fixtureSnapshot } from '../test/fixtures';

let tell: (status: SaveStatus) => void = () => undefined;
vi.mock('../bridge/commands', () => ({
  bridge: {
    onSaveStatus: (handler: typeof tell) => {
      tell = handler;
      return Promise.resolve(() => undefined);
    },
  },
}));
const { useProject } = await import('./project');
const { useToasts } = await import('./toasts');
const { followSaves, saveState, useSaving } = await import('./saving');

const opened = (revision: number, opening = 1) => {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, opening, history: { ...snapshot.history, revision } };
};

/** Each test's follower, stopped after it, so none hears the next test's project. */
let following: (() => void) | null = null;
const follow = async () => (following = await followSaves());
afterEach(() => following?.());

beforeEach(() => {
  useProject.setState({ snapshot: null, pending: 0 });
  useSaving.setState({ failing: null, saved: null });
  useToasts.setState({ toasts: [] });
});

describe('save state', () => {
  it('a project just opened is saved; an edit is saving until its revision lands', async () => {
    await follow();
    useProject.setState({ snapshot: opened(3) });
    expect(saveState()).toBe('saved');
    useProject.setState({ snapshot: opened(4) });
    expect(saveState()).toBe('saving');
    tell({ ok: true, opening: 1, revision: 4 });
    expect(saveState()).toBe('saved');
    // A save of another opening (the same project closed and opened again) says nothing about this one.
    useProject.setState({ snapshot: opened(1, 2) });
    useProject.setState({ snapshot: opened(2, 2) });
    tell({ ok: true, opening: 1, revision: 2 });
    expect(saveState()).toBe('saving');
  });

  it('failing saves say why and stay; the save that works again says so', async () => {
    await follow();
    useProject.setState({ snapshot: opened(3) });
    useProject.setState({ snapshot: opened(4) });
    tell({ ok: false, message: "Your disk is full, so changes aren't being saved. Free up some space." });
    expect(saveState()).toBe('failing');
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.persistent])).toEqual([['error', true]]);
    tell({ ok: true, opening: 1, revision: 4 });
    expect(saveState()).toBe('saved');
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['Your changes are saved again.']);
  });

  it('a late save of the last opening never puts a project just opened back on Saving', async () => {
    await follow();
    useProject.setState({ snapshot: opened(4) });
    // Another project opened before the last one's final save reported.
    useProject.setState({ snapshot: opened(0, 2) });
    expect(saveState()).toBe('saved');
    tell({ ok: true, opening: 1, revision: 4 });
    expect(saveState()).toBe('saved');
  });

  it('stopping leaves the store alone: no subscription stays behind', async () => {
    (await follow())();
    following = null;
    useProject.setState({ snapshot: opened(3) });
    expect(useSaving.getState().saved).toBeNull();
  });
});
