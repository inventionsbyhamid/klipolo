import { useLingui } from '@lingui/react/macro';
import { useEffect, useId, useState } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import type { ProjectRow } from '../../bindings/ProjectRow';
import { projects } from '../../bridge/projects';
import { Button } from '../../design/Button';
import { useProject } from '../../state/project';
import { ProjectCard } from './ProjectCard';
import './start.css';

/** A new project: "Untitled project" in all three formats, 16:9 first. */
const NEW_PROJECT: { name: string; formats: FormatKind[] } = {
  name: 'Untitled project',
  formats: ['landscape', 'portrait', 'square'],
};

/** The start screen's own icons, on the design's 24 px grid (1.8 px stroke, `currentColor`). */
const ICONS = {
  // A ring with a filled 8 px dot: in the button's ink, never red.
  record: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <circle className="ico-fill" cx="12" cy="12" r="4" />
    </>
  ),
  alert: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7.5V13M12 16.3v.1" />
    </>
  ),
};

function StartIcon({ name }: { name: keyof typeof ICONS }) {
  return (
    <svg className="ico" viewBox="0 0 24 24" aria-hidden>
      {ICONS[name]}
    </svg>
  );
}

type Load = { status: 'loading' } | { status: 'ready'; rows: ProjectRow[] } | { status: 'failed' };

const noop = () => undefined;

interface StartScreenProps {
  /** Record your screen: opens the Record dialog. */
  onRecord?: () => void;
}

/**
 * Where Klipolo opens: your projects, last opened first, with New project (the one red) and Record your
 * screen. A card opens its project; New project makes "Untitled project" in all three formats and opens it. Shown while
 * no project is open.
 */
export function StartScreen({ onRecord = noop }: StartScreenProps) {
  const { t } = useLingui();
  const titleId = useId();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let alive = true;
    projects.recent().then(
      (rows) => {
        if (alive) setLoad({ status: 'ready', rows });
      },
      () => {
        if (alive) setLoad({ status: 'failed' });
      },
    );
    return () => {
      alive = false;
    };
  }, [attempt]);

  const tryAgain = () => {
    setLoad({ status: 'loading' });
    setAttempt((n) => n + 1);
  };

  // One project at a time: a second press while one is opening (or being made) would open twice; the first one wins.
  const busy = () => useProject.getState().pending > 0;
  const openProject = (path: string) => {
    if (!busy()) void useProject.getState().open(path);
  };
  const newProject = () => {
    if (!busy()) void useProject.getState().create(NEW_PROJECT.name, [...NEW_PROJECT.formats]);
  };

  const actions = (
    <div className="start-actions">
      <Button variant="primary" onClick={newProject}>
        {t({ id: 'start.new', message: 'New project' })}
      </Button>
      <Button variant="secondary" onClick={() => onRecord()}>
        <StartIcon name="record" />
        {t({ id: 'start.record', message: 'Record your screen' })}
      </Button>
    </div>
  );
  const empty = load.status === 'ready' && load.rows.length === 0;
  const nowMs = Date.now();

  return (
    <div className="panel start-panel">
      <main className="start" aria-labelledby={titleId}>
        <header className="start-head">
          <h1 id={titleId} className="start-title">
            {t({ id: 'start.title', message: 'Your projects' })}
          </h1>
          {/* While loading there is nothing to choose yet; with no projects the buttons sit under the invitation. */}
          {(load.status === 'failed' || (load.status === 'ready' && !empty)) && actions}
        </header>
        {load.status === 'loading' && (
          <div className="start-note" role="status">
            <span className="spinner" aria-hidden />
            {t({ id: 'start.loading', message: 'Loading your projects' })}
          </div>
        )}
        {load.status === 'failed' && (
          <div className="start-note">
            <p className="start-said" role="alert">
              <StartIcon name="alert" />
              {t({
                id: 'start.failed',
                message: "Klipolo couldn't read your projects folder. Check the disk, then try again.",
              })}
            </p>
            <Button variant="secondary" onClick={tryAgain}>
              {t({ id: 'start.tryAgain', message: 'Try again' })}
            </Button>
          </div>
        )}
        {empty && (
          <div className="start-empty">
            <h2 className="start-title">{t({ id: 'start.empty.title', message: 'No projects yet' })}</h2>
            <p>{t({ id: 'start.empty.text', message: 'Start with a video or a recording of your screen.' })}</p>
            {actions}
          </div>
        )}
        {load.status === 'ready' && !empty && (
          // WebKit drops the list role of a list drawn without bullets: it is said again.
          <ul className="start-grid" role="list">
            {load.rows.map((row) => (
              <ProjectCard key={row.path} row={row} nowMs={nowMs} onOpen={openProject} />
            ))}
          </ul>
        )}
      </main>
    </div>
  );
}
