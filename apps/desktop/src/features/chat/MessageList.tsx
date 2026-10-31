import { useLingui } from '@lingui/react/macro';
import { Fragment, useLayoutEffect, useRef } from 'react';
import type { Asset } from '../../bindings/Asset';
import { openSettings, useAi } from '../../ai/aiStatus';
import { NeedsKey } from '../../ai/NeedsKey';
import { Icon } from '../../layout/Icon';
import { offerAgain, stateOf, useAnalysis, useSomeNotAnalysed } from '../../state/analysis';
import { useCanChat, useChat } from '../../state/chat';
import { useChatPanel } from '../../state/chatPanel';
import { useProject } from '../../state/project';
import { ChatMessage } from './ChatMessage';

/** What an empty chat suggests, each sent as it reads. */
const SUGGESTIONS = [
  'cut the ums and long pauses',
  'make a 60 second highlight',
  'make it vertical with karaoke captions',
  'add captions',
  'find the best moments',
];

const NO_ASSETS: Asset[] = [];

/**
 * Where the project's clips are with analysis, for the empty chat's note: some still being heard (their words still to
 * come), some only being watched (the scene log, last: their words are in, so cuts work), some not analysed (never,
 * failed or stopped), or every one done (or no clips).
 */
function useAnalysisNote(): 'listening' | 'watching' | 'missing' | null {
  const assets = useProject((s) => s.snapshot?.project.assets ?? NO_ASSETS);
  return useAnalysis((s) => {
    const states = assets
      .filter((a) => a.kind === 'video' || a.kind === 'audio')
      .map((a) => stateOf(s.states, a.id));
    if (states.some((a) => a.state === 'running' && a.step !== 'scenes')) return 'listening';
    if (states.some((a) => a.state === 'running')) return 'watching';
    if (states.some((a) => a.state !== 'done')) return 'missing';
    return null;
  });
}

/**
 * Why chat is off with a key or code saved (an access code the relay turned off): the AI status's own sentence, with
 * the way to Settings. Nothing otherwise (no key or code shows the NeedsKey card).
 */
function ChatOff() {
  const { t } = useLingui();
  const notice = useAi((s) => (s.status?.mode === 'relay' ? (s.status.notice ?? null) : null));
  if (!notice) return null;
  return (
    <p className="note" role="status">
      <Icon name="info" />
      <span>{notice}</span>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        style={{ gridColumn: 2, justifySelf: 'start' }}
        onClick={openSettings}
      >
        {t({ id: 'chatPanel.empty.settings', message: 'Open Settings' })}
      </button>
    </p>
  );
}

/**
 * "Ask for an edit", its suggestions (greyed while chat can't run), the NeedsKey card when there is no key or code, why
 * chat is off when an access code was turned off, and a note while the clips aren't analysed or are being analysed
 *.
 */
function EmptyChat() {
  const { t } = useLingui();
  const allowed = useCanChat();
  const analysis = useAnalysisNote();
  const offerable = useSomeNotAnalysed();
  return (
    <div className="chat-empty">
      <h3>{t({ id: 'chatPanel.empty.title', message: 'Ask for an edit' })}</h3>
      <p>
        {t({
          id: 'chatPanel.empty.body',
          message: 'Type it like a message. Klipolo asks before it cuts anything.',
        })}
      </p>
      {!allowed && <NeedsKey />}
      {!allowed && <ChatOff />}
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
      {allowed && analysis && (
        <p className="note">
          <Icon name="info" />
          <span>
            {analysis === 'listening'
              ? t({
                  id: 'chatPanel.empty.analysing',
                  message: 'Klipolo is still listening to these clips. Cuts and moments work when analysis is done.',
                })
              : analysis === 'watching'
                ? t({
                    id: 'chatPanel.empty.watching',
                    message:
                      'Klipolo is still watching these clips. Cutting ums and pauses works now. Moments work when analysis is done.',
                  })
                : t({
                    id: 'chatPanel.empty.notAnalysed',
                    message:
                      'Analyze the clips first to cut pauses, find moments and add captions. Format and music work now.',
                  })}
          </span>
          {analysis === 'missing' && offerable && (
            // The offer comes back in the analysis banner, with its price, even after "Not now".
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              style={{ gridColumn: 2, justifySelf: 'start' }}
              onClick={() => void offerAgain()}
            >
              {t({ id: 'chatPanel.empty.analyze', message: 'Analyze' })}
            </button>
          )}
        </p>
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
