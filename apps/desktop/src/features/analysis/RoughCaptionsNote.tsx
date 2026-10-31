import type { AssetState } from '../../bindings/AssetState';
import { retry, useAnalysis } from '../../state/analysis';
import './analysis.css';

/**
 * The rough note: its sentence, and the clips "Time words again" asks for (joined by spaces, so what the store gives
 * is a string that stays the same while nothing changes).
 */
interface Rough {
  text: string;
  again: string;
}

const LEAD = 'Some captions are timed roughly.';

/**
 * The preview header's note on rough captions: while a clip's words are being timed, its captions show
 * each turn's words spread across it ("Rough captions until words are timed", with a spinner); once its analysis is
 * done with some words still timed roughly, the note says so, and why when timing them again can help (word timing
 * failed, stopped or didn't finish), with "Time words again" for those clips (free; the person asks, so Rust times them
 * whatever it did before). Words that don't match their sound can't be helped: the note says only that they are rough.
 * Nothing otherwise. Only the open project's clips count (the store forgets the last project's).
 */
export function RoughCaptionsNote() {
  const timing = useAnalysis((s) =>
    Object.values(s.states).some((state) => state.state === 'running' && state.step === 'words'),
  );
  const text = useAnalysis((s) => rough(s.states)?.text ?? null);
  const again = useAnalysis((s) => rough(s.states)?.again ?? '');
  if (timing) {
    return (
      <span className="stage-status analysis-rough">
        <span className="spinner" aria-hidden />
        <span className="analysis-rough-text">Rough captions until words are timed</span>
      </span>
    );
  }
  if (text === null) return null;
  return (
    <span className="stage-status analysis-rough">
      <span className="analysis-rough-text" title={text}>
        {text}
      </span>
      {again && (
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => void retry(again.split(' '))}>
          Time words again
        </button>
      )}
    </span>
  );
}

/**
 * The done clips with rough words: the note's sentence (each reason timing them again can help with, once), and the
 * clips timing them again can help. None when no clip is rough.
 */
function rough(states: Record<string, AssetState>): Rough | null {
  const clips = Object.entries(states).flatMap(([id, state]) =>
    state.state === 'done' && state.approximate ? [{ id, state }] : [],
  );
  if (clips.length === 0) return null;
  const helped = clips.filter((c) => c.state.timeAgain);
  const reasons = [...new Set(helped.flatMap((c) => (c.state.approximateReason ? [c.state.approximateReason] : [])))];
  return { text: [LEAD, ...reasons].join(' '), again: helped.map((c) => c.id).join(' ') };
}
