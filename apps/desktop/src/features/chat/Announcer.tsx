import { useLingui } from '@lingui/react/macro';
import { useChat } from '../../state/chat';

/**
 * Says politely what chat answered, once, and what waits for the person. Outside the part that folds. A failure is said
 * by its own message (`role="alert"`).
 */
export function Announcer() {
  const { t } = useLingui();
  const last = useChat((s) => s.messages[s.messages.length - 1]);
  const pending = useChat((s) => s.pending !== null);
  let text = '';
  if (pending) text = t({ id: 'chatPanel.announce.thinking', message: 'Klipolo is thinking.' });
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
