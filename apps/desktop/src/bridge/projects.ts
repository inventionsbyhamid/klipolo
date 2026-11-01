import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ProjectPoster } from '../bindings/ProjectPoster';
import type { ProjectRow } from '../bindings/ProjectRow';
import { call } from './call';

/** The start screen's and the project menu's commands (`crates/klipolo-app/src/project.rs`). */
export const projects = {
  /**
   * The projects in `~/Movies/Klipolo`, last opened first (at most 30): each one's name, length and picture.
   * Reads the files; opens nothing.
   */
  recent: () => call<ProjectRow[]>('projects_recent'),
  /**
   * Each card's poster as it lands: a listing asks for the posters its cards lack, and Rust makes them one at a time
   * in the background. Until the returned function is called.
   */
  onPoster: (handler: (poster: ProjectPoster) => void): Promise<UnlistenFn> =>
    listen<ProjectPoster>('projects://poster', (event) => handler(event.payload)),
  /** Shows the open project's file in Finder, selected. */
  reveal: () => call<void>('project_reveal'),
};
