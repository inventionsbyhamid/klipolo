import type { ProjectRow } from '../bindings/ProjectRow';
import { call } from './call';

/** The start screen's command (`crates/klipolo-app/src/project.rs`). */
export const projects = {
  /**
   * The projects in `~/Movies/Klipolo`, last opened first (at most 30): each one's name, length and first thumbnail.
   * Reads the files; opens nothing.
   */
  recent: () => call<ProjectRow[]>('projects_recent'),
};
