import { useAnalysis } from '../../state/analysis';
import './analysis.css';

/**
 * The preview header's note on rough captions: while a clip's words are being timed, its captions show
 * each turn's words spread across it ("Rough captions until words are timed", with a spinner); once its analysis is
 * done with some words still timed roughly (word timing failed or stopped; they are timed again on the next run),
 * the note says so. Nothing otherwise. Only the open project's clips count (the store forgets the last project's).
 */
export function RoughCaptionsNote() {
  const note = useAnalysis((s) => {
    const states = Object.values(s.states);
    if (states.some((state) => state.state === 'running' && state.step === 'words')) return 'timing';
    if (states.some((state) => state.state === 'done' && state.approximate)) return 'rough';
    return null;
  });
  if (note === 'timing') {
    return (
      <span className="stage-status analysis-rough">
        <span className="spinner" aria-hidden />
        Rough captions until words are timed
      </span>
    );
  }
  if (note === 'rough') return <span className="stage-status analysis-rough">Some captions are timed roughly</span>;
  return null;
}
