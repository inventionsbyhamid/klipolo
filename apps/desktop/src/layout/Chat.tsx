import { useEffect, useRef } from 'react';
import { useUi } from '../state/ui';
import { Icon } from './Icon';

/**
 * The chat panel, as a placeholder: the empty state and a composer that waits for chat to land. Command K puts the
 * cursor in the composer (`useUi.focusChat`).
 */
export function Chat() {
  const composer = useRef<HTMLTextAreaElement>(null);
  const chatFocus = useUi((s) => s.chatFocus);
  useEffect(() => {
    if (chatFocus > 0) composer.current?.focus();
  }, [chatFocus]);
  return (
    <aside className="panel chat" aria-label="Chat">
      <div className="chat-head">
        <h2>Chat</h2>
        <button className="icon-btn" aria-label="New chat" title="New chat" disabled>
          <Icon name="new-chat" />
        </button>
        <button className="icon-btn" aria-label="Collapse chat" title="Collapse chat" disabled>
          <Icon name="collapse" />
        </button>
      </div>
      <div className="log">
        <div className="chat-empty">
          <h3>Ask for an edit</h3>
          <p>Type it like a message. Klipolo asks before it cuts anything.</p>
        </div>
      </div>
      <div className="composer">
        <div className="input-wrap">
          <textarea ref={composer} rows={1} placeholder="Say what you want" disabled aria-label="Message" />
          <button className="send" disabled aria-label="Send">
            <Icon name="send" />
          </button>
        </div>
      </div>
    </aside>
  );
}
