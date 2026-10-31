import { useLingui } from '@lingui/react/macro';
import { Fragment, useLayoutEffect, useRef } from 'react';
import { NeedsKey } from '../../ai/NeedsKey';
import { useAi } from '../../ai/aiStatus';
import { chatAllowed, useChat } from '../../state/chat';
import { useChatPanel } from '../../state/chatPanel';
import { ChatMessage } from './ChatMessage';

/** What an empty chat suggests, each sent as it reads. */
const SUGGESTIONS = [
  'cut the ums and long pauses',
  'make a 60 second highlight',
  'make it vertical with karaoke captions',
  'add captions',
  'find the best moments',
];

/** "Ask for an edit", its suggestions (greyed while chat can't run), and the NeedsKey card when there is no key or code. */
function EmptyChat() {
  const { t } = useLingui();
  const allowed = useAi((s) => chatAllowed(s.status));
  return (
    <div className="chat-empty">
      <h3>{t({ id: 'chatPanel.empty.title', message: 'Ask for an edit' })}</h3>
      <p>
        {t({
          id: 'chatPanel.empty.body',
          message: 'Type it like a message. Klipolo asks before it cuts anything.',
        })}
      </p>
      <NeedsKey />
      {allowed ? (
        <div className="suggestions">
          {SUGGESTIONS.map((s) => (
            <button key={s} type="button" className="chip chip-outline" onClick={() => void useChat.getState().send(s)}>
              {s}
            </button>
          ))}
        </div>
      ) : (
        <div className="suggestions is-off" aria-hidden>
          {SUGGESTIONS.slice(0, 3).map((s) => (
            <span key={s} className="chip">
              {s}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The conversation, newest in view: also when chat opens, for a reply that came while it was folded (a folded list has
 * no box to scroll). The log itself is quiet: `Announcer` says each reply once, politely. While Klipolo answers, the
 * thinking dots show under the person's message.
 */
export function MessageList() {
  const { t } = useLingui();
  const messages = useChat((s) => s.messages);
  const pending = useChat((s) => s.pending !== null);
  const folded = useChatPanel((s) => s.collapsed);
  const end = useRef<HTMLDivElement>(null);
  // Before the frame is drawn: chat opened never shows the top first.
  useLayoutEffect(() => {
    if (!folded) end.current?.scrollIntoView?.({ block: 'nearest' });
  }, [messages.length, pending, folded]);
  return (
    <div
      className="log"
      role="log"
      aria-live="off"
      aria-label={t({ id: 'chatPanel.log', message: 'Conversation', comment: 'The chat’s list of messages' })}
      tabIndex={0}
      data-chat-body-focus
    >
      {messages.length === 0 && !pending ? (
        <EmptyChat />
      ) : (
        <>
          {messages.map((m) => (
            <Fragment key={`${m.from}:${m.id}`}>
              <ChatMessage message={m} />
            </Fragment>
          ))}
          {pending && (
            <p className="thinking">
              <i aria-hidden>
                <b />
                <b />
                <b />
              </i>
              {t({ id: 'chatPanel.thinking', message: 'Thinking' })}
            </p>
          )}
        </>
      )}
      <div ref={end} />
    </div>
  );
}
