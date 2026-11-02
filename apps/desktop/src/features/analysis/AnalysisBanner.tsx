import { useEffect } from 'react';
import type { Asset } from '../../bindings/Asset';
import type { Project } from '../../bindings/Project';
import { openSettings, useAi, useNeedsKey } from '../../ai/aiStatus';
import { recordingOf, voiceOf } from '../../lib/analysed';
import { assetName, recordingName } from '../../lib/timeline';
import {
  analyze,
  cancelAnalysis,
  notNow,
  offerFor,
  retry,
  stateOf,
  useAnalysedClips,
  useAnalysis,
  useTakes,
} from '../../state/analysis';
import { useProject } from '../../state/project';
import { Icon } from '../../layout/Icon';
import {
  FILE_MISSING,
  STEP_LABELS,
  counted,
  missingTitle,
  offerText,
  offerTitle,
  progressTitle,
  spokenProgress,
  stepDots,
  takeState,
} from './analysis';
import './analysis.css';

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

/**
 * What analysis offers (`useAnalysedClips`): the files the project's main-track clips play, a recording counted as one
 * clip by its voice.
 */
const useClips = useAnalysedClips;

const NO_ASSETS: Asset[] = [];
const NO_RECORDINGS: Project['recordings'] = [];

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

/**
 * A row's name: a recording's voice is the take, named as the timeline names the take's clip ("Recording 1"); a
 * recording's file placed on its own as that clip is ("Screen of Recording 1"); any other file by its own name. The
 * row and what a screen reader hears of it say the same.
 */
function rowName(project: Project | undefined, clip: Asset): string {
  if (!project) return clip.name;
  const take = voiceOf(project, clip.id);
  return take ? recordingName(project, take) : assetName(project, clip);
}

/** A clip's art and name, as each row of the banner starts (`rowName`): a take has a film's art. */
function ClipName({ clip }: { clip: Asset }) {
  const take = useProject((s) => (s.snapshot ? voiceOf(s.snapshot.project, clip.id) : null));
  const name = useProject((s) => rowName(s.snapshot?.project, clip));
  const film = clip.kind === 'video' || take !== null;
  return (
    <>
      <span className={film ? 'art-clip' : 'art-clip is-sound'} aria-hidden>
        {film ? <Icon name="film" /> : <BannerIcon name="wave" />}
      </span>
      <span className="analysis-job-name" title={name}>
        {name}
      </span>
    </>
  );
}

/** The clips whose file Klipolo can't find (moved or deleted): left out of the offer, each with what to do. */
function MissingRows({ clips }: { clips: Asset[] }) {
  return (
    <div className="analysis-jobs">
      {clips.map((clip) => (
        <div key={clip.id} className="analysis-job">
          <ClipName clip={clip} />
          <span className="analysis-job-reason">{FILE_MISSING}</span>
        </div>
      ))}
    </div>
  );
}

/** `clips` as the banner's rows, in order: a take's files (`takes`) are one row, a clip of its own. */
function rowsOf(clips: Asset[], takes: ReadonlyMap<string, string>): Asset[][] {
  const rows = new Map<string, Asset[]>();
  for (const clip of clips) {
    const key = takes.get(clip.id) ?? clip.id;
    rows.set(key, [...(rows.get(key) ?? []), clip]);
  }
  return [...rows.values()];
}

/**
 * One clip's row while its analysis runs or after it failed, a take's files as one (`takeState`): its step and step
 * dots, or why it stopped, with Retry for its files that failed.
 */
function JobRow({ files }: { files: Asset[] }) {
  const state = useAnalysis((s) => takeState(files.map((f) => stateOf(s.states, f.id))));
  const again = () => {
    const { states } = useAnalysis.getState();
    void retry(files.filter((f) => stateOf(states, f.id).state === 'failed').map((f) => f.id));
  };
  return (
    <div className="analysis-job">
      <ClipName clip={files[0]} />
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
          <button
            type="button"
            // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
            tabIndex={0}
            className="btn btn-ghost btn-sm"
            onClick={again}
          >
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
 * keeps what finished and offers Retry. A take counts as one clip throughout, whatever files its voice is in. Without
 * a key or code it asks for one in Settings. A clip whose file is gone is never offered: it is listed under the offer
 * with what to do.
 */
export function AnalysisBanner() {
  const clips = useClips();
  useOffer(clips);
  const states = useAnalysis((s) => s.states);
  const estimate = useAnalysis((s) => s.estimate);
  const error = useAnalysis((s) => s.error);
  const dismissed = useAnalysis((s) => s.dismissed);
  const busy = useAnalysis((s) => s.busy);
  const takes = useTakes();
  const assets = useProject((s) => s.snapshot?.project.assets ?? NO_ASSETS);
  const recordings = useProject((s) => s.snapshot?.project.recordings ?? NO_RECORDINGS);
  const needsKey = useNeedsKey();

  const at = (files: Asset[]) => takeState(files.map((f) => stateOf(states, f.id)));
  const rows = rowsOf(
    clips.filter((c) => ['running', 'failed'].includes(stateOf(states, c.id).state)),
    takes,
  );
  const running = rows.filter((files) => at(files).state === 'running');
  const failed = rows.filter((files) => at(files).state === 'failed');
  const spoken = useProject((s) =>
    running
      .map((files) => {
        const state = at(files);
        const name = rowName(s.snapshot?.project, files[0]);
        return state.state === 'running' ? spokenProgress(name, state.step, state.pct) : '';
      })
      .join('. '),
  );
  if (running.length > 0 || failed.length > 0) {
    return (
      <section className="analysis-banner is-progress" aria-label="Analysis">
        <p className="analysis-title">
          {running.length > 0 && <span className="spinner" aria-hidden />}
          {progressTitle(running.length, failed.length)}
        </p>
        {running.length > 0 && (
          <button type="button" tabIndex={0} className="btn btn-ghost btn-sm" onClick={cancelAnalysis}>
            Cancel
          </button>
        )}
        {error && (
          <p className="analysis-text analysis-error" role="alert">
            {error}
          </p>
        )}
        <div className="analysis-jobs">
          {[...running, ...failed].map((files) => (
            <JobRow key={files[0].id} files={files} />
          ))}
        </div>
        <p className="sr-only" role="status">
          {spoken}
        </p>
      </section>
    );
  }

  const offered = estimate?.clips.filter((id) => !dismissed.includes(id)) ?? [];
  const gone = clips.filter((c) => estimate?.missing.includes(c.id) && !dismissed.includes(c.id));
  if (!estimate || (offered.length === 0 && gone.length === 0)) return null;
  const missing = gone.length > 0 && <MissingRows clips={gone} />;
  if (offered.length === 0) {
    // Only clips whose file is gone: what to do for each.
    return (
      <section className="analysis-banner is-progress" aria-label="Analysis">
        <p className="analysis-title">{missingTitle(gone.length)}</p>
        <button type="button" tabIndex={0} className="btn btn-ghost btn-sm" onClick={notNow}>
          Not now
        </button>
        {missing}
      </section>
    );
  }
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
          <button type="button" tabIndex={0} className="btn btn-ghost" onClick={notNow}>
            Not now
          </button>
          <button type="button" tabIndex={0} className="btn btn-secondary" onClick={openSettings}>
            Open Settings
          </button>
        </div>
        {missing}
      </section>
    );
  }
  // Footage is watched only for a video imported on its own: a recording's files are analysed for their words only.
  const videos = clips.some(
    (c) => c.kind === 'video' && !recordingOf({ assets, recordings }, c.id) && estimate.clips.includes(c.id),
  );
  const refused = !!estimate.refusal;
  // Retry asks for the estimate again (the price may have changed), never starts a run unseen.
  const pending = clips.filter((c) => stateOf(states, c.id).state === 'none').map((c) => c.id);
  const again = () => void offerFor(pending);
  if (estimate.problem) {
    // Nothing can pay yet: why, and no price.
    return (
      <section className="analysis-banner" aria-label="Analysis">
        <span className="analysis-ico">
          <BannerIcon name="wave" />
        </span>
        <div>
          <p className="analysis-title">{estimate.problem}</p>
        </div>
        <div className="analysis-actions">
          <button type="button" tabIndex={0} className="btn btn-ghost" onClick={notNow}>
            Not now
          </button>
          <button type="button" tabIndex={0} className="btn btn-secondary" disabled={busy} onClick={again}>
            <BannerIcon name="retry" />
            Retry
          </button>
        </div>
        {missing}
      </section>
    );
  }
  return (
    <section className="analysis-banner" aria-label="Analysis">
      <span className="analysis-ico">
        <BannerIcon name="wave" />
      </span>
      <div>
        <p className="analysis-title">{offerTitle(counted(estimate, takes))}</p>
        <p className="analysis-text">{error ?? offerText(counted(estimate, takes), videos)}</p>
      </div>
      <div className="analysis-actions">
        <button type="button" tabIndex={0} className="btn btn-ghost" onClick={notNow}>
          Not now
        </button>
        {error ? (
          <button type="button" tabIndex={0} className="btn btn-secondary" disabled={busy} onClick={again}>
            <BannerIcon name="retry" />
            Retry
          </button>
        ) : (
          <button
            type="button"
            tabIndex={0}
            className="btn btn-primary"
            disabled={busy || refused}
            onClick={() => void analyze()}
          >
            Analyze
          </button>
        )}
      </div>
      {missing}
    </section>
  );
}
