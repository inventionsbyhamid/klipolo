import { useAnalysis } from '../../state/analysis';
import './analysis.css';

/**
 * The preview header's note while a clip's words are being timed: its captions show each turn's words
 * spread across it until then. Nothing otherwise.
 */
export function RoughCaptionsNote() {
  const timing = useAnalysis((s) =>
    Object.values(s.states).some((state) => state.state === 'running' && state.step === 'words'),
  );
  if (!timing) return null;
  return (
    <span className="stage-status analysis-rough">
      <span className="spinner" aria-hidden />
      Rough captions until words are timed
    </span>
  );
}
