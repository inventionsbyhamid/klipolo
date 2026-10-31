import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, useState } from 'react';
import type { ItemContent } from '../../bindings/ItemContent';
import { registerBurst } from '../../state/bursts';
import { queueCommit } from '../../state/commit';
import { useProject } from '../../state/project';
import { findText, MAX_TEXT, useTextEdit } from '../preview/textEdit';

type TextContent = Extract<ItemContent, { type: 'text' }>;

interface TextFieldProps {
  /** The text layer's id. */
  id: string;
  content: TextContent;
  /** The layer's track is locked, so its words can't change. */
  locked: boolean;
}

/**
 * The selected title's words, at the top of the inspector. Enter or leaving the field saves ("Edit text" in history),
 * Shift Enter starts a new line and Escape puts the saved words back. Remount it (via `key`) whenever the saved text
 * changes. While the same words are typed on the canvas it edits that one draft instead: it shows and changes the
 * canvas's words, and Enter or Escape finishes the shared edit, keeping them; the edit commits once.
 *
 * Words typed here and not yet saved go in as the project closes (or the app quits), in their turn after any change
 * on its way, built on the title as the project has it then.
 */
export function TextField({ id, content, locked }: TextFieldProps) {
  const { t } = useLingui();
  const session = useTextEdit((s) => (s.session?.id === id ? s.session : null));
  const [draft, setDraft] = useState(content.text);
  const value = session ? session.text : draft;
  const typed = draft.trim();
  // The words typed here and not yet saved (while the canvas types them, the shared edit commits them instead).
  const unsaved = !session && !locked && typed && typed !== content.text ? typed : null;
  const pending = useRef<string | null>(null);
  pending.current = unsaved;

  const send = (text: string) => {
    pending.current = null;
    const label = 'text.edit.undo';
    // Built at its turn on the title as the project has it then: a change to it on its way stays.
    return queueCommit(
      label,
      () => {
        const now = findText(useProject.getState().snapshot?.project, id)?.item.content;
        return now?.type === 'text' ? [{ type: 'updateItem', id, patch: { content: { ...now, text } } }] : [];
      },
      undefined,
      { keep: label },
    ).landed;
  };

  // A close (or quit) with words typed here and not left: they go in, kept at close.
  useEffect(
    () =>
      registerBurst((closing) => {
        const words = pending.current;
        if (!closing || words === null) return false;
        void send(words);
        return true;
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- registered once; reads the newest words by ref
    [],
  );

  function commit() {
    const words = pending.current;
    if (words === null) {
      setDraft(content.text);
      return;
    }
    // A failed edit leaves the saved text unchanged (no remount), so put it back in the field.
    void send(words).then((ok) => {
      if (!ok) setDraft(content.text);
    });
  }

  return (
    <div className="text-field">
      <textarea
        aria-label={t({ id: 'text.field', message: 'Text', comment: 'The words of the selected title' })}
        className="input input-sm textarea"
        rows={2}
        value={value}
        maxLength={MAX_TEXT}
        readOnly={locked}
        // Part of the canvas edit: pressing here doesn't finish it.
        data-text-edit={session ? '' : undefined}
        onChange={(e) => (session ? useTextEdit.getState().setText(e.target.value) : setDraft(e.target.value))}
        onCompositionStart={() => {
          if (session) useTextEdit.getState().setComposing(true);
        }}
        onCompositionEnd={() => {
          if (session) useTextEdit.getState().setComposing(false);
        }}
        onBlur={() => {
          // The shared edit commits when it finishes, not when this field lets go.
          if (!session) commit();
        }}
        onKeyDown={(e) => {
          // Typing here never reaches the editor's shortcuts.
          e.stopPropagation();
          const composing = e.nativeEvent.isComposing || e.keyCode === 229;
          if (session) {
            if (!composing && (e.key === 'Escape' || (e.key === 'Enter' && !e.shiftKey))) {
              e.preventDefault();
              void useTextEdit.getState().finish();
            }
            return;
          }
          if (e.key === 'Enter' && !e.shiftKey && !composing) {
            e.preventDefault();
            e.currentTarget.blur();
          }
          if (e.key === 'Escape') {
            e.preventDefault();
            // The saved words back, and nothing to commit as it lets go.
            pending.current = null;
            setDraft(content.text);
            e.currentTarget.blur();
          }
        }}
      />
      {locked && (
        <p className="field-note">
          {t({ id: 'text.field.locked', message: 'Unlock its track to edit the text.' })}
        </p>
      )}
    </div>
  );
}
