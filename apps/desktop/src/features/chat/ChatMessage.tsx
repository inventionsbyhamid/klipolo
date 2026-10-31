import { useLingui } from '@lingui/react/macro';
import { memo } from 'react';
import { openSettings } from '../../ai/aiStatus';
import { SpriteIcon } from '../../design/SpriteIcon';
import { useChat, type Message } from '../../state/chat';
import { Moments } from './Moments';
import { ProposalCard } from './ProposalCard';
import { UndoChip } from './UndoChip';

/** Failures the person fixes in Settings, not by trying again. */
const SETTINGS = new Set(['budget', 'refused', 'needsKey']);

/**
 * One message: the person's in a paper bubble on the right; Klipolo's as plain text (already without em
 * en dashes: Rust cleaned it), then what couldn't be done, its questions, the Undo chip under additions it applied,
 * its proposal card and its moments. A failure says what happened, with Retry (busy, network, a server error, this Mac
 * not registered) or Open Settings (credit, a refused key or code); a message too long for chat, or one the AI declined
 * (its words are back in the box to reword), offers neither. The project is untouched.
 */
export const ChatMessage = memo(function ChatMessage({ message: m }: { message: Message }) {
  const { t } = useLingui();
  if (m.from === 'person') return <p className="msg-you">{m.text}</p>;
  if (m.kind === 'failed')
    return (
      // Said by the announcer, outside the part that folds.
      <div className="msg-k">
        <p>{m.text}</p>
        <div className="msg-row">
          {SETTINGS.has(m.failure) ? (
            <button type="button" className="chip chip-outline" onClick={openSettings}>
              {t({ id: 'chatPanel.openSettings', message: 'Open Settings' })}
            </button>
          ) : m.failure === 'tooLong' || m.failure === 'declined' ? null : (
            <button type="button" className="chip chip-outline" onClick={() => void useChat.getState().retry(m.id)}>
              <SpriteIcon name="retry" />
              {t({ id: 'chatPanel.retry', message: 'Retry' })}
            </button>
          )}
        </div>
      </div>
    );
  return (
    <div className="msg-k">
      {m.text && <p>{m.text}</p>}
      {m.notes.map((n, i) => (
        <p key={`n${i}`} className="muted">
          {n}
        </p>
      ))}
      {m.questions.map((q, i) => (
        <p key={`q${i}`}>{q}</p>
      ))}
      {m.applied && <UndoChip undo={m.applied.undo} revision={m.applied.revision} />}
      {m.proposal && <ProposalCard message={m} />}
      <Moments moments={m.moments} stale={m.momentsStale} />
    </div>
  );
});
