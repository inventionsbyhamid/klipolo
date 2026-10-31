import { useLingui } from '@lingui/react/macro';
import { useChat } from '../../state/chat';

/**
 * Says politely what chat answered, once, a failure too, what waits for the person, and a sentence chat said outside a
 * message (an Undo chip refused: `notice`). Outside the part that folds, so it speaks while chat is folded.
 */
export function Announcer() {
  const { t } = useLingui();
  const last = useChat((s) => s.messages[s.messages.length - 1]);
  const pending = useChat((s) => s.pending !== null);
  const notice = useChat((s) => s.notice);
  let text = '';
  if (pending) text = t({ id: 'chatPanel.announce.thinking', message: 'Klipolo is thinking.' });
  else if (notice) text = notice;
  else if (last?.from === 'klipolo' && last.kind === 'failed')
    text = t({ id: 'chatPanel.announce.says', message: `Klipolo: ${last.text}` });
  else if (last?.from === 'klipolo' && last.kind === 'reply') {
    if (last.proposal?.state === 'open') {
      const title = last.proposal.proposal.title;
      text = t({ id: 'chatPanel.announce.review', message: `Needs your review: ${title}` });
    } else text = t({ id: 'chatPanel.announce.says', message: `Klipolo: ${last.text}` });
  }
  return (
    <p aria-live="polite" className="sr-only">
      {text}
    </p>
  );
}
