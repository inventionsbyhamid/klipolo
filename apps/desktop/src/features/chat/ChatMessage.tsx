import { useLingui } from '@lingui/react/macro';
import { memo } from 'react';
import { openSettings } from '../../ai/aiStatus';
import { SpriteIcon } from '../../design/SpriteIcon';
import { useChat, type Message } from '../../state/chat';
import { appLine } from '../../state/chatApps';
import { Moments } from './Moments';
import { PictureCard } from './PictureCard';
import { ProposalCard } from './ProposalCard';
import { UndoChip } from './UndoChip';

/** Failures the person fixes in Settings, not by trying again. */
const SETTINGS = new Set(['budget', 'refused', 'needsKey']);

/**
 * One message: the person's in a paper bubble on the right; Klipolo's as plain text (already without em
 * or en dashes: Rust cleaned it), then what couldn't be done, its questions, the Undo chip under additions it applied,
 * its proposal card, the pictures it asked for and its moments. A failure says what happened, with Retry (busy,
 * network, a server error, this Mac not registered) or Open Settings (credit, a refused key or code); a message too
 * long for chat, one the AI declined (its words are back in the box to reword), or one whose times the project left (it
 * says to ask again), offers neither. The project is untouched. Another app's reply has no words of its own: "From
 * {client}" over it, then what it did, and the same chips and cards.
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
          ) : m.failure === 'tooLong' || m.failure === 'declined' || m.retry === null ? null : (
            <button type="button" className="chip chip-outline" onClick={() => void useChat.getState().retry(m.id)}>
              <SpriteIcon name="retry" />
              {t({ id: 'chatPanel.retry', message: 'Retry' })}
            </button>
          )}
        </div>
      </div>
    );
  const client = m.client;
  const line = client !== undefined ? appLine(m) : '';
  return (
    <div className="msg-k">
      {client !== undefined && (
        <p className="msg-from">
          {t({ id: 'mcp.from', message: `From ${client}`, comment: 'Over what another app did: {client} is the app' })}
        </p>
      )}
      {m.text && <p>{m.text}</p>}
      {line && <p>{line}</p>}
      {m.notes.map((n, i) => (
        <p key={`n${i}`} className="muted">
          {n}
        </p>
      ))}
      {m.questions.map((q, i) => (
        <p key={`q${i}`}>{q}</p>
      ))}
      {m.applied && <UndoChip undo={m.applied.undo} revision={m.applied.revision} byApp={client !== undefined} />}
      {m.proposal && <ProposalCard message={m} />}
      {m.pictures.map((part) => (
        <PictureCard key={part.picture.id} messageId={m.id} part={part} />
      ))}
      <Moments moments={m.moments} revision={m.revision} stale={m.momentsStale} />
    </div>
  );
});
