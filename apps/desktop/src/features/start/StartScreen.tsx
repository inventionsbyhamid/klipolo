import { useLingui } from '@lingui/react/macro';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
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

/** What the screen shows: the list on its way, the list, or that it couldn't be read (`retrying`: asked again). */
type Load =
  | { status: 'loading' }
  | { status: 'ready'; rows: ProjectRow[] }
  | { status: 'failed'; retrying: boolean };

const noop = () => undefined;

interface StartScreenProps {
  /** Record your screen: opens the Record dialog. */
  onRecord?: () => void;
}

/**
 * Where Klipolo opens: your projects, last opened first, with New project (the one red) and Record your
 * screen. A card opens its project (and the list is read again when it can't be opened: it may have moved); New project
 * makes "Untitled project" in all three formats and opens it. Shown while no project is open.
 */
export function StartScreen({ onRecord = noop }: StartScreenProps) {
  const { t } = useLingui();
  const titleId = useId();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [opening, setOpening] = useState<string | null>(null);
  const mounted = useRef(true);
  const mainRef = useRef<HTMLElement>(null);
  const tryAgainRef = useRef<HTMLButtonElement>(null);
  // Try again had the focus when the list landed: it goes to the first card (or New project), not to the page.
  const refocus = useRef(false);

  /** Reads the list; what is shown stays until it lands. */
  const list = useCallback(() => {
    projects.recent().then(
      (rows) => {
        if (!mounted.current) return;
        refocus.current = tryAgainRef.current !== null && document.activeElement === tryAgainRef.current;
        setLoad({ status: 'ready', rows });
      },
      () => {
        if (mounted.current) setLoad({ status: 'failed', retrying: false });
      },
    );
  }, []);

  useEffect(() => {
    mounted.current = true;
    list();
    return () => {
      mounted.current = false;
    };
  }, [list]);

  useEffect(() => {
    if (!refocus.current || load.status !== 'ready') return;
    refocus.current = false;
    mainRef.current?.querySelector<HTMLElement>('.start-grid .start-card, .start-empty .btn-primary')?.focus();
  }, [load]);

  // Try again stays where it is, inert with a spinner while the list is asked for again, so the focus stays on it.
  const retrying = load.status === 'failed' && load.retrying;
  const tryAgain = () => {
    if (load.status !== 'failed' || load.retrying) return;
    setLoad({ status: 'failed', retrying: true });
    list();
  };

  // One project at a time: a second press while one is opening (or being made) would open twice; the first one wins.
  const busy = () => useProject.getState().pending > 0;
  const openProject = (path: string) => {
    if (busy()) return;
    setOpening(path);
    void useProject
      .getState()
      .open(path)
      .then((opened) => {
        if (!mounted.current) return;
        setOpening(null);
        // It couldn't be opened (the store says why): the list as it is now.
        if (!opened) list();
      });
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
      <main className="start" aria-labelledby={titleId} ref={mainRef}>
        <header className="start-head">
          <h1 id={titleId} className="start-title">
            {t({ id: 'start.title', message: 'Your projects' })}
          </h1>
          {/* With no projects the buttons sit under the invitation instead. */}
          {!empty && actions}
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
            <Button
              ref={tryAgainRef}
              variant="secondary"
              aria-disabled={retrying || undefined}
              aria-busy={retrying || undefined}
              onClick={tryAgain}
            >
              {retrying && <span className="spinner" aria-hidden />}
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
              <ProjectCard
                key={row.path}
                row={row}
                nowMs={nowMs}
                busy={opening === row.path}
                onOpen={openProject}
              />
            ))}
          </ul>
        )}
      </main>
    </div>
  );
}
