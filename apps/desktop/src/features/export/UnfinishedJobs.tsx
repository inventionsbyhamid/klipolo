import { useEffect, useRef } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { unfinishedJobs, useExport } from '../../state/export';
import { useProject } from '../../state/project';
import { UnfinishedJobRow } from './UnfinishedJobRow';

/**
 * Unfinished exports at the top of the export dialog, to resume or discard: quiet rows, never a dialog of their own.
 * This project's come first. When a row goes (Discard, Start over), focus goes to the row now in its place, else
 * `onEmptied` takes it.
 */
export function UnfinishedJobs({ onEmptied }: { onEmptied: () => void }) {
  const projectId = useProject((s) => s.snapshot?.project.id);
  const jobs = useExport(useShallow(unfinishedJobs));
  const list = useRef<HTMLUListElement>(null);
  /** The row whose Discard or Start over was pressed: when it's gone, focus goes to the row now in its place. */
  const leaving = useRef<{ id: string; index: number } | null>(null);
  const ordered = [
    ...jobs.filter((j) => j.projectId === projectId),
    ...jobs.filter((j) => j.projectId !== projectId),
  ];
  const ids = ordered.map((j) => j.jobId).join('\n');

  useEffect(() => {
    const left = leaving.current;
    if (!left || ids.split('\n').includes(left.id)) return;
    leaving.current = null;
    // Only when focus went with the row: a view change (Start over) moves it itself.
    if (document.activeElement && document.activeElement !== document.body) return;
    const rows = list.current?.querySelectorAll('li') ?? [];
    const next = rows[Math.min(left.index, rows.length - 1)]?.querySelector('button');
    if (next) next.focus();
    else onEmptied();
  }, [ids, onEmptied]);

  if (ordered.length === 0) return null;
  return (
    <section className="ex-unfinished" aria-labelledby="export-unfinished">
      <h3 className="ex-label" id="export-unfinished">
        Unfinished exports
      </h3>
      <ul ref={list} className="ex-jobs">
        {ordered.map((job, index) => (
          <UnfinishedJobRow
            key={job.jobId}
            job={job}
            here={job.projectId === projectId}
            onLeave={() => (leaving.current = { id: job.jobId, index })}
          />
        ))}
      </ul>
    </section>
  );
}
