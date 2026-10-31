import { useLingui } from '@lingui/react/macro';
import { useChat } from '../../state/chat';

/**
 * Says politely what chat answered, once, a failure too, and what waits for the person. Beside it, in a place of its
 * own, what chat says outside a message (`notice`: an Undo chip refused, a picture ready or placed), so a message
 * thinking or its reply never keeps a notice from being heard. Outside the part that folds, so it speaks while chat is
 * folded.
 */
export function Announcer() {
  const { t } = useLingui();
  const last = useChat((s) => s.messages[s.messages.length - 1]);
  const pending = useChat((s) => s.pending !== null);
  const notice = useChat((s) => s.notice);
  let text = '';
  if (pending) text = t({ id: 'chatPanel.announce.thinking', message: 'Klipolo is thinking.' });
  else if (last?.from === 'klipolo' && last.kind === 'failed')
    text = t({ id: 'chatPanel.announce.says', message: `Klipolo: ${last.text}` });
  else if (last?.from === 'klipolo' && last.kind === 'reply') {
    if (last.proposal?.state === 'open') {
      const title = last.proposal.proposal.title;
      text = t({ id: 'chatPanel.announce.review', message: `Needs your review: ${title}` });
    } else text = t({ id: 'chatPanel.announce.says', message: `Klipolo: ${last.text}` });
  }
  return (
    <>
      <p aria-live="polite" className="sr-only">
        {text}
      </p>
      <p aria-live="polite" className="sr-only" data-chat-notices>
        {/* A new node for each notice, so the same words said again are announced again. */}
        {notice && <span key={notice.seq}>{notice.text}</span>}
      </p>
    </>
  );
}
