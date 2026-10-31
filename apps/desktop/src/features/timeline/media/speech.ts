import { create } from 'zustand';
import type { Project } from '../../../bindings/Project';
import type { RangeUs } from '../../../bindings/RangeUs';
import { library } from '../../../bridge/library';
import { useProject } from '../../../state/project';

/**
 * Where someone speaks on the timeline, as the engine's mix ducks music under it (`media_speech_ranges`): the Music
 * lane's volume line dips there. Asked again whenever the project changes while a sound layer ducks, and when the
 * speech itself does (`refreshSpeech`: a clip's analysis finished).
 */
export const useSpeech = create<{ ranges: RangeUs[] }>(() => ({ ranges: [] }));

const NONE: RangeUs[] = [];

/** Whether any sound layer ducks under speech (by more than 0 dB): only then are there dips to draw. */
function ducks(project: Project): boolean {
  return project.timeline.tracks.some((t) =>
    t.items.some((i) => i.content.type === 'audio' && (i.content.duck ?? 0) < 0),
  );
}

let asked = 0;

/** Asks Rust where someone speaks now; the newest answer wins. */
export function refreshSpeech() {
  const project = useProject.getState().snapshot?.project;
  const ask = ++asked;
  if (!project || !ducks(project)) {
    if (useSpeech.getState().ranges !== NONE) useSpeech.setState({ ranges: NONE });
    return;
  }
  void library
    .speechRanges()
    .then((ranges) => {
      if (ask === asked) useSpeech.setState({ ranges: ranges.length ? ranges : NONE });
    })
    .catch(() => undefined);
}

let following = false;

/** Keeps the ranges current as the project changes (once per page). */
export function followSpeech() {
  if (following) return;
  following = true;
  useProject.subscribe((s, prev) => {
    const now = s.snapshot;
    const was = prev.snapshot;
    if (now?.history.revision === was?.history.revision && now?.project.id === was?.project.id) return;
    refreshSpeech();
  });
  refreshSpeech();
}
