import { useLingui } from '@lingui/react/macro';
import { useEffect, useState } from 'react';
import type { CodedError } from '../../bindings/CodedError';
import type { HistoryState } from '../../bindings/HistoryState';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import { historyAction } from '../../lib/history';
import { useProject } from '../../state/project';
import { useReturnFocus } from './focus';

/** How long "Redo" shows after an Undo. */
const REDO_MS = 10_000;

/**
 * Where a chat edit (`undo` made `revision`) stands in the history: still the latest change (Command Z would take it
 * back), taken back just after it was the latest (each change, undo included, is a new revision: so it was this one,
 * not an older edit with the same label), or buried under later changes.
 */
export function standing(
  h: HistoryState | undefined,
  undo: CodedError,
  revision: number,
): 'latest' | 'undone' | 'buried' {
  if (h?.canUndo && h.revision === revision && h.undoLabel === undo.code) return 'latest';
  return h?.canRedo && h.revision === revision + 1 && h.redoLabel === undo.code ? 'undone' : 'buried';
}

/**
 * Undo for a chat edit: an outline chip with the undo icon. It undoes only while that edit is still the
 * latest change, checked again when the undo's turn comes (a change still settling goes in first, and then this one is
 * no longer the latest: `useProject.undoIf`). Taken back, it reads "Redo" for 10 s, then goes. Buried under later
 * changes, it does nothing and says why. When it changes under the keyboard, the keyboard goes back to the chat box.
 */
export function UndoChip({ undo, revision }: { undo: CodedError; revision: number }) {
  const { t } = useLingui();
  const [busy, setBusy] = useState(false);
  const state = useProject((s) => standing(s.snapshot?.history, undo, revision));
  const [redoGone, setRedoGone] = useState(false);
  useEffect(() => {
    if (state !== 'undone') return setRedoGone(false);
    const timer = setTimeout(() => setRedoGone(true), REDO_MS);
    return () => clearTimeout(timer);
  }, [state]);
  const focusProps = useReturnFocus(state === 'latest' || (state === 'undone' && !redoGone));
  if (state === 'undone') {
    if (redoGone) return null;
    return (
      <div className="msg-row" {...focusProps}>
        <button
          type="button"
          className="chip chip-outline"
          aria-label={historyAction('redo', undo.code, undo.params)}
          aria-disabled={busy || undefined}
          onClick={() => {
            if (busy) return;
            setBusy(true);
            const latest = useProject.getState().snapshot?.history;
            if (standing(latest, undo, revision) !== 'undone') return setBusy(false);
            void useProject
              .getState()
              .redo()
              .finally(() => setBusy(false));
          }}
        >
          <Icon name="redo" />
          {t({ id: 'chatPanel.redo', message: 'Redo' })}
        </button>
      </div>
    );
  }
  const name = historyAction('undo', undo.code, undo.params);
  const chip = (
    <button
      type="button"
      className="chip chip-outline"
      aria-label={name}
      aria-disabled={busy || state === 'buried' || undefined}
      onClick={() => {
        // One press takes back one change: a second before the first lands would take back the one before.
        if (busy || state !== 'latest') return;
        setBusy(true);
        void useProject
          .getState()
          .undoIf((h) => standing(h, undo, revision) === 'latest')
          .finally(() => setBusy(false));
      }}
    >
      <Icon name="undo" />
      {t({ id: 'chatPanel.undo', message: 'Undo' })}
    </button>
  );
  return (
    <div className="msg-row" {...focusProps}>
      {state === 'buried' ? (
        <Tooltip
          label={t({
            id: 'chatPanel.undo.buried',
            message: 'Other changes came after this one. Undo those first.',
          })}
          describes
        >
          {chip}
        </Tooltip>
      ) : (
        chip
      )}
    </div>
  );
}
