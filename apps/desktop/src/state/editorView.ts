import type { EditorView } from '../bindings/EditorView';
import { sendEditorView } from '../bridge/mcp';
import { findItem } from '../lib/timeline';
import { usePlayback } from './playback';
import { useProject } from './project';
import { useRange } from './range';
import { useSelection } from './selection';
import { useUi } from './ui';

/**
 * What "this" and "here" mean now: the time range selected, the playhead, the overlay selected (a clip selected is
 * none) and the format the preview shows. Chat's messages carry it, and other apps read it from Klipolo.
 */
export function currentView(): EditorView {
  const project = useProject.getState().snapshot?.project;
  const range = useRange.getState().range;
  const selected = useSelection.getState().selectedId;
  return {
    selection: range ? { start: range.startUs, end: range.endUs } : null,
    playheadUs: usePlayback.getState().playheadUs,
    item: project && selected && findItem(project, selected) ? selected : null,
    format: useUi.getState().previewFormat ?? project?.settings.primaryFormat ?? null,
  };
}

/** The view, to Klipolo once it settles (`sendEditorView`), while a project is open. */
function send() {
  if (useProject.getState().snapshot) sendEditorView(currentView());
}

/**
 * Tells Klipolo the person's view as it settles, for other apps' calls: after the range selection changes, a seek or a
 * pause (the playhead moving while it plays says nothing), the overlay selected or the format shown changes, and each
 * time a project opens (Klipolo forgets the view on every opening). Returns the stop.
 */
export function followEditorView(): () => void {
  const stops = [
    useRange.subscribe((s, was) => {
      if (s.range !== was.range) send();
    }),
    usePlayback.subscribe((s, was) => {
      // A seek moves the sequence; stopped, the playhead moves only where a pause, a seek or the end put it.
      if (s.seq !== was.seq || (!s.playing && (was.playing || s.playheadUs !== was.playheadUs))) send();
    }),
    useSelection.subscribe((s, was) => {
      if (s.selectedId !== was.selectedId) send();
    }),
    useUi.subscribe((s, was) => {
      if (s.previewFormat !== was.previewFormat) send();
    }),
    useProject.subscribe((s, was) => {
      const [now, before] = [s.snapshot, was.snapshot];
      if (now && (now.project.id !== before?.project.id || now.opening !== before?.opening)) send();
    }),
  ];
  return () => stops.forEach((stop) => stop());
}
