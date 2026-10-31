import { useLingui } from '@lingui/react/macro';
import { useLayoutEffect, useRef, useState } from 'react';
import { useAi } from '../../ai/aiStatus';
import { codedMessage } from '../../bridge/call';
import { cn } from '../../design/cn';
import { Icon } from '../../layout/Icon';
import { useCanChat, useChat } from '../../state/chat';
import { useProject } from '../../state/project';
import { SelectionChip } from './SelectionChip';

/** The box grows with what is typed, up to six lines. */
const MAX_LINES = 6;
const LINE_PX = 20;

/**
 * The chat box: Enter sends and Shift Enter adds a line (Enter while an input method composes a word
 * confirms the word). The send button turns yellow when there is text. While Klipolo answers, the box stays usable:
 * Enter keeps the message and says it waits. It works only while the AI status allows chat (a key, or an access code
 * the relay hasn't turned off): otherwise it is off, and says to add a key or code when there is none. The selection chip
 * above it shows what "this" means. Command K puts the cursor here (the panel's `focusChat`).
 */
export function Composer() {
  const { t } = useLingui();
  // In the chat store, so an unsent message outlives the panel folding.
  const text = useChat((s) => s.draft);
  const setText = useChat((s) => s.setDraft);
  const pending = useChat((s) => s.pending !== null);
  const allowed = useCanChat();
  const none = useAi((s) => s.status?.mode === 'none') && !allowed;
  const readOnly = useProject((s) => s.snapshot?.readOnly ?? null);
  const open = useProject((s) => s.snapshot !== null);
  const field = useRef<HTMLTextAreaElement>(null);
  const [focused, setFocused] = useState(false);
  /** Enter was pressed while Klipolo was still answering: the message stays, and the box says why. */
  const [held, setHeld] = useState(false);
  const disabled = !allowed || readOnly !== null || !open;
  const canSend = !disabled && !pending && text.trim() !== '';
  useLayoutEffect(() => {
    const box = field.current;
    if (!box) return;
    box.style.height = 'auto';
    box.style.height = `${Math.min(box.scrollHeight, MAX_LINES * LINE_PX + 16)}px`;
  }, [text]);
  const send = () => {
    if (pending && text.trim() !== '') setHeld(true);
    if (!canSend) return;
    setHeld(false);
    setText('');
    void useChat.getState().send(text);
  };
  return (
    <div className="composer">
      {readOnly !== null && <p className="composer-note">{codedMessage({ code: readOnly, params: {} }) || readOnly}</p>}
      <SelectionChip />
      <div className={cn('input-wrap', disabled && 'is-off')}>
        <textarea
          ref={field}
          data-chat-composer=""
          aria-label={t({ id: 'chatPanel.message', message: 'Message', comment: 'The chat message field' })}
          rows={1}
          value={text}
          disabled={disabled}
          placeholder={
            none
              ? t({ id: 'chatPanel.message.needsKey', message: 'Add a key or code to chat' })
              : t({ id: 'chatPanel.message.placeholder', message: 'Say what you want' })
          }
          onChange={(e) => setText(e.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.metaKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              send();
            }
          }}
        />
        {!disabled && !focused && text === '' && <kbd aria-hidden>⌘K</kbd>}
        <button
          type="button"
          className="send"
          aria-label={t({ id: 'chatPanel.send', message: 'Send' })}
          disabled={!canSend}
          onClick={() => {
            send();
            field.current?.focus();
          }}
        >
          <Icon name="send" />
        </button>
      </div>
      <p role="status" className="composer-note">
        {held &&
          pending &&
          t({
            id: 'chatPanel.held',
            message: 'Klipolo is still answering. Send this once it is done.',
            comment: 'Under the chat box after Enter while a reply is on its way; the typed message stays in the box',
          })}
      </p>
    </div>
  );
}
