import { useSpeech } from '../features/timeline/media/speech';
import { mainIndex } from '../features/timeline/model/index';
import { useProject } from '../state/project';

const S = 1_000_000;

/**
 * Where someone might speak in the open project: a few seconds of talk with short breaks, over its clips. Development
 * only, until a clip's analysis feeds the engine its speech (the Music lane then asks Rust for the real ranges): it lets
 * the volume line's dips be seen. The page only: the mix itself ducks under the engine's own ranges.
 */
function sampleSpeech() {
  const snapshot = useProject.getState().snapshot;
  if (!snapshot) return [];
  const { mainEndUs } = mainIndex(snapshot.project.timeline.main);
  const ranges = [];
  for (let at = 2 * S, k = 0; at + S < mainEndUs; k++) {
    const end = Math.min(mainEndUs, at + (1.5 + (k % 3)) * S);
    ranges.push({ start: at, end });
    at = end + (3 + (k % 3)) * S;
  }
  return ranges;
}

/** Control Shift S shows sample speech under the music, or takes it away (development builds only). */
export function listenForSampleSpeech(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (!e.ctrlKey || !e.shiftKey || e.code !== 'KeyS') return;
    e.preventDefault();
    const shown = useSpeech.getState().ranges.length > 0;
    useSpeech.setState({ ranges: shown ? [] : sampleSpeech() });
  };
  window.addEventListener('keydown', onKey);
  return () => window.removeEventListener('keydown', onKey);
}
