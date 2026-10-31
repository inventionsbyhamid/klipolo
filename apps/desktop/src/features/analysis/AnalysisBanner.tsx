import { useEffect, useMemo } from 'react';
import type { Asset } from '../../bindings/Asset';
import { openSettings, useAi, useNeedsKey } from '../../ai/aiStatus';
import {
  analyze,
  cancelAnalysis,
  notNow,
  offerFor,
  retry,
  stateOf,
  useAnalysis,
} from '../../state/analysis';
import { Icon } from '../../layout/Icon';
import { useProject } from '../../state/project';
import {
  STEP_LABELS,
  offerText,
  offerTitle,
  progressTitle,
  spokenProgress,
  stepDots,
} from './analysis';
import './analysis.css';

const NO_ASSETS: Asset[] = [];

/** The icons the banner needs, from the design's sprite (24 px grid, 1.8 px stroke). */
const ICONS = {
  wave: <path d="M4 10v4M8 7v10M12 4v16M16 8v8M20 11v2" />,
  key: (
    <>
      <circle cx="8" cy="15" r="4" />
      <path d="m11 12 8.5-8.5M16.5 6.5 19 9M14 9l2 2" />
    </>
  ),
  retry: (
    <>
      <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3" />
      <path d="M19.5 4.5v4.5H15" />
    </>
  ),
};

function BannerIcon({ name }: { name: keyof typeof ICONS }) {
  return (
    <svg className="ico" viewBox="0 0 24 24" aria-hidden>
      {ICONS[name]}
    </svg>
  );
}

/** The project's videos and sounds: what analysis is for. */
function useClips(): Asset[] {
  const assets = useProject((s) => s.snapshot?.project.assets ?? NO_ASSETS);
  return useMemo(() => assets.filter((a) => a.kind === 'video' || a.kind === 'audio'), [assets]);
}

/**
 * Asks for the offer whenever the clips not analysed change (an import, another project, a Cancel) or the way to pay
 * does (a key added, a code's credit known).
 */
function useOffer(clips: Asset[]) {
  const mode = useAi((s) => s.status?.mode);
  const ids = useAnalysis((s) =>
    clips
      .filter((c) => stateOf(s.states, c.id).state === 'none')
      .map((c) => c.id)
      .join(' '),
  );
  useEffect(() => {
    if (ids) void offerFor(ids.split(' '));
  }, [ids, mode]);
}

/** One clip's row while its analysis runs or after it failed: its step and step dots, or why it stopped. */
function JobRow({ clip }: { clip: Asset }) {
  const state = useAnalysis((s) => stateOf(s.states, clip.id));
  return (
    <div className="analysis-job">
      <span className={clip.kind === 'video' ? 'art-clip' : 'art-clip is-sound'} aria-hidden>
        {clip.kind === 'video' ? <Icon name="film" /> : <BannerIcon name="wave" />}
      </span>
      <span className="analysis-job-name" title={clip.name}>
        {clip.name}
      </span>
      {state.state === 'running' && (
        <>
          <span className="analysis-job-step">
            {STEP_LABELS[state.step]}
            <span className="analysis-steps" aria-hidden>
              {stepDots(state.step, state.pct).map((dot, i) => (
                <i
                  key={i}
                  className={dot.kind === 'next' ? undefined : dot.kind}
                  style={dot.kind === 'now' ? ({ '--p': `${dot.pct}%` } as React.CSSProperties) : undefined}
                />
              ))}
            </span>
          </span>
          <span className="analysis-job-pct">{state.pct}%</span>
        </>
      )}
      {state.state === 'failed' && (
        <>
          <span className="analysis-job-reason" role="alert">
            {state.reason}
          </span>
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => void retry([clip.id])}>
            <BannerIcon name="retry" />
            Retry
          </button>
        </>
      )}
    </div>
  );
}

/**
 * The analysis banner at the top of the preview. After an import it offers "Analyze 2 clips for about
 * $0.04" (clips analyzed before load at once, at no cost); with an access code it shows the credit left and refuses,
 * with why, when the estimate is over it. While clips are analysed it lists each with its step; a clip that failed
 * keeps what finished and offers Retry. Without a key or code it asks for one in Settings.
 */
export function AnalysisBanner() {
  const clips = useClips();
  useOffer(clips);
  const states = useAnalysis((s) => s.states);
  const estimate = useAnalysis((s) => s.estimate);
  const error = useAnalysis((s) => s.error);
  const dismissed = useAnalysis((s) => s.dismissed);
  const busy = useAnalysis((s) => s.busy);
  const needsKey = useNeedsKey();

  const running = clips.filter((c) => stateOf(states, c.id).state === 'running');
  const failed = clips.filter((c) => stateOf(states, c.id).state === 'failed');
  if (running.length > 0 || failed.length > 0) {
    const spoken = running
      .map((c) => {
        const s = stateOf(states, c.id);
        return s.state === 'running' ? spokenProgress(c.name, s.step, s.pct) : '';
      })
      .join('. ');
    return (
      <section className="analysis-banner is-progress" aria-label="Analysis">
        <p className="analysis-title">
          {running.length > 0 && <span className="spinner" aria-hidden />}
          {progressTitle(running.length, failed.length)}
        </p>
        {running.length > 0 && (
          <button type="button" className="btn btn-ghost btn-sm" onClick={cancelAnalysis}>
            Cancel
          </button>
        )}
        {error && (
          <p className="analysis-text analysis-error" role="alert">
            {error}
          </p>
        )}
        <div className="analysis-jobs">
          {[...running, ...failed].map((clip) => (
            <JobRow key={clip.id} clip={clip} />
          ))}
        </div>
        <p className="sr-only" role="status">
          {spoken}
        </p>
      </section>
    );
  }

  const offered = estimate?.clips.filter((id) => !dismissed.includes(id)) ?? [];
  if (!estimate || offered.length === 0) return null;
  if (needsKey) {
    return (
      <section className="analysis-banner" aria-label="Analysis">
        <span className="analysis-ico">
          <BannerIcon name="key" />
        </span>
        <div>
          <p className="analysis-title">Add your Meta API key or access code in Settings</p>
          <p className="analysis-text">
            Transcripts, captions and finding moments use Meta's models. Editing and export work without them.
          </p>
        </div>
        <div className="analysis-actions">
          <button type="button" className="btn btn-ghost" onClick={notNow}>
            Not now
          </button>
          <button type="button" className="btn btn-secondary" onClick={openSettings}>
            Open Settings
          </button>
        </div>
      </section>
    );
  }
  const videos = clips.some((c) => c.kind === 'video' && estimate.clips.includes(c.id));
  const refused = !!estimate.refusal;
  return (
    <section className="analysis-banner" aria-label="Analysis">
      <span className="analysis-ico">
        <BannerIcon name="wave" />
      </span>
      <div>
        <p className="analysis-title">{offerTitle(estimate)}</p>
        <p className="analysis-text">{error ?? offerText(estimate, videos)}</p>
      </div>
      <div className="analysis-actions">
        <button type="button" className="btn btn-ghost" onClick={notNow}>
          Not now
        </button>
        {error ? (
          <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => void analyze()}>
            <BannerIcon name="retry" />
            Retry
          </button>
        ) : (
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy || refused}
            onClick={() => void analyze()}
          >
            Analyze
          </button>
        )}
      </div>
    </section>
  );
}
