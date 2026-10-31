import { useEffect, useState } from 'react';
import type { ClipFraming } from '../../bindings/ClipFraming';
import { clipFraming } from '../../bridge/framing';
import { useProject } from '../../state/project';

/** Rust's answer for one clip. */
interface Answer {
  clipId: string;
  framing: ClipFraming;
}

/**
 * Rust's framing of clip `clipId` in every format (`clipFraming`: its own, else the default, follow included), asked
 * again whenever the project changes. The last answer stays on screen while the next is on its way; null until the
 * first. A clip that's gone (removed since) answers nothing. One small call per revision while the clip is selected.
 */
export function useClipFraming(clipId: string): ClipFraming | null {
  const revision = useProject((s) => s.snapshot?.history.revision ?? -1);
  const projectId = useProject((s) => s.snapshot?.project.id ?? null);
  const [answer, setAnswer] = useState<Answer | null>(null);
  useEffect(() => {
    let live = true;
    clipFraming(clipId).then(
      (framing) => {
        if (live && framing) setAnswer({ clipId, framing });
      },
      // Gone, or no project: nothing to show.
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [clipId, revision, projectId]);
  return answer?.clipId === clipId ? answer.framing : null;
}
