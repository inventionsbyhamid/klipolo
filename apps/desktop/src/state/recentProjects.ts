import { create } from 'zustand';
import type { ProjectRow } from '../bindings/ProjectRow';
import type { Snapshot } from '../bindings/Snapshot';
import { projects } from '../bridge/projects';
import { useProject } from './project';

/** The most projects Open recent lists. */
export const OPEN_RECENT_MAX = 5;

/**
 * A list read this recently is still the list as the pointer comes back to the name or the menu opens again (a read
 * parses up to 30 project files); an older one is read again. A project that went since says so when picked.
 */
const FRESH_MS = 10_000;

/**
 * Open recent, in the project menu and the commands: up to `OPEN_RECENT_MAX` projects from the start screen's list
 * (`projects.recent`), last opened first, the one open now left out. Read while a project is open, when the menu is
 * about to open (the pointer or the focus on the project's name) and as it opens: never at launch or on a project
 * switch, so nothing is read for a menu nobody opens.
 */
interface RecentProjects {
  rows: ProjectRow[];
  /** Which opening of which project they were read for (`openingOf`); null before any. */
  readFor: string | null;
  /** When they were read (`Date.now()`). */
  readAt: number;
}

export const useRecentProjects = create<RecentProjects>(() => ({ rows: [], readFor: null, readAt: 0 }));

/** Which opening of which project `snapshot` is: a list read for another one doesn't leave this one out. */
function openingOf(snapshot: Snapshot | null): string | null {
  return snapshot ? `${snapshot.project.id}:${snapshot.opening ?? ''}` : null;
}

/** The read under way, for the opening it is for. */
let reading: { key: string; done: Promise<void> } | null = null;

/**
 * Reads the list for the project open now (a read already under way for it is the one). One that can't be read leaves
 * the list as it was.
 */
export function readRecent(): Promise<void> {
  const key = openingOf(useProject.getState().snapshot);
  if (key === null) return Promise.resolve();
  if (reading?.key === key) return reading.done;
  const read = { key, done: Promise.resolve() };
  read.done = (async () => {
    try {
      const rows = await projects.recent();
      // Another project opened meanwhile: this list isn't its list.
      if (openingOf(useProject.getState().snapshot) !== key) return;
      useRecentProjects.setState({
        rows: rows.filter((row) => !row.open).slice(0, OPEN_RECENT_MAX),
        readFor: key,
        readAt: Date.now(),
      });
    } catch {
      // The menu goes on without the group, or with the list as it was; the start screen says why it can't be read.
    } finally {
      if (reading === read) reading = null;
    }
  })();
  reading = read;
  return read.done;
}

/** Reads the list unless it was read for the project open now within the last moment. */
export function readRecentIfStale(): void {
  const { readFor, readAt } = useRecentProjects.getState();
  if (readFor !== openingOf(useProject.getState().snapshot) || Date.now() - readAt > FRESH_MS) void readRecent();
}

/** Open recent's projects for the project open now, as read so far: null until its list has been read. */
export function openRecentNow(): ProjectRow[] | null {
  const { rows, readFor } = useRecentProjects.getState();
  return readFor !== null && readFor === openingOf(useProject.getState().snapshot) ? rows : null;
}

const NONE: ProjectRow[] = [];

/** Open recent's projects for the project open now: none until its list has been read. */
export function useOpenRecent(): ProjectRow[] {
  const key = useProject((s) => openingOf(s.snapshot));
  const rows = useRecentProjects((s) => s.rows);
  const readFor = useRecentProjects((s) => s.readFor);
  return readFor !== null && readFor === key ? rows : NONE;
}

/**
 * Opens a project from Open recent as a start screen card does: not while another create, open or close is under way
 * (the first one wins); one that can't be opened (moved or deleted since: the store says why) has the list read again,
 * and the project open stays open.
 */
export async function openRecent(path: string): Promise<boolean> {
  const project = useProject.getState();
  if (project.switching) return false;
  const opened = await project.open(path);
  if (!opened) await readRecent();
  return opened;
}
