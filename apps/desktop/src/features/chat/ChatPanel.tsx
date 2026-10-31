import { useLingui } from '@lingui/react/macro';
import { useEffect, useLayoutEffect, useRef } from 'react';
import { SpriteIcon } from '../../design/SpriteIcon';
import { Icon } from '../../layout/Icon';
import { openProposal } from '../../state/chatMessages';
import { useChat } from '../../state/chat';
import { useChatPanel } from '../../state/chatPanel';
import { useUi } from '../../state/ui';
import { Announcer } from './Announcer';
import { Composer } from './Composer';
import { focusComposer } from './focus';
import { MessageList } from './MessageList';
import './chat.css';

/** The chat's body, which the strip and the header's Collapse open and fold. */
export const CHAT_BODY_ID = 'chat-body';

/**
 * The chat panel: the header (Chat, New chat, Collapse), the conversation and the composer. It folds to a
 * 56 px strip, never hidden: an Open button and the chat icon, with a red dot while a proposal waits. Folded, the panel
 * stays mounted (`hidden` and `inert`): a reply on its way still lands, and the announcer (outside the folded part)
 * still speaks. A fold with the keyboard inside puts it on the strip; Command K (`useUi.focusChat`) opens chat with the
 * cursor in the composer, and Command Enter (the editor's `apply-proposal`) applies the proposal that is open.
 */
export function ChatPanel() {
  const { t } = useLingui();
  const collapsed = useChatPanel((s) => s.collapsed);
  const waiting = useChat((s) => openProposal(s.messages) !== null);
  const strip = useRef<HTMLButtonElement>(null);
  const inside = useRef(false);
  /** The composer takes the keyboard once the body shows (opened by the strip or Command K). */
  const wantFocus = useRef(false);

  // Command K counts up for the whole session: only a press made while this is on screen opens it.
  const asked = useUi((s) => s.chatFocus);
  const seen = useRef(asked);
  useEffect(() => {
    if (asked === seen.current) return;
    seen.current = asked;
    wantFocus.current = true;
    useChatPanel.getState().setCollapsed(false);
  }, [asked]);
  useLayoutEffect(() => {
    if (collapsed && inside.current) strip.current?.focus();
    if (!collapsed && wantFocus.current) {
      wantFocus.current = false;
      focusComposer();
    }
  });
  const expand = () => {
    wantFocus.current = true;
    useChatPanel.getState().setCollapsed(false);
  };
  return (
    <aside className="panel chat" aria-label={t({ id: 'chatPanel.title', message: 'Chat' })} data-collapsed={collapsed}>
      {collapsed && (
        <div className="chat-strip">
          <button
            ref={strip}
            type="button"
            className="icon-btn"
            aria-label={t({ id: 'chatPanel.open', message: 'Open chat' })}
            title={t({ id: 'chatPanel.open.tip', message: 'Open chat (Command K)' })}
            aria-expanded={false}
            aria-controls={CHAT_BODY_ID}
            onClick={expand}
          >
            <SpriteIcon name="expand-panel" />
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label={
              waiting
                ? t({ id: 'chatPanel.waiting', message: 'Chat, 1 proposal waiting' })
                : t({ id: 'chatPanel.title', message: 'Chat' })
            }
            onClick={expand}
          >
            <SpriteIcon name="chat" />
            {waiting && <span className="badge-dot" aria-hidden />}
          </button>
        </div>
      )}
      <div
        id={CHAT_BODY_ID}
        className="chat-body"
        hidden={collapsed}
        inert={collapsed}
        onFocus={() => (inside.current = true)}
        onBlur={(e) => {
          const left = e.target;
          if (e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
          // Checked once the event is over: a control still focused only lost the window's focus (Command Tab).
          queueMicrotask(() => {
            if (left.isConnected && document.activeElement !== left) inside.current = false;
          });
        }}
      >
        <div className="chat-head">
          <h2>{t({ id: 'chatPanel.title', message: 'Chat' })}</h2>
          <button
            type="button"
            className="icon-btn"
            aria-label={t({ id: 'chatPanel.new', message: 'New chat' })}
            title={t({ id: 'chatPanel.new', message: 'New chat' })}
            onClick={() => {
              useChat.getState().newChat();
              focusComposer();
            }}
          >
            <Icon name="new-chat" />
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label={t({ id: 'chatPanel.collapse', message: 'Collapse chat' })}
            title={t({ id: 'chatPanel.collapse', message: 'Collapse chat' })}
            aria-expanded
            aria-controls={CHAT_BODY_ID}
            onClick={() => useChatPanel.getState().setCollapsed(true)}
          >
            <Icon name="collapse" />
          </button>
        </div>
        <MessageList />
        <Composer />
      </div>
      <Announcer />
    </aside>
  );
}
