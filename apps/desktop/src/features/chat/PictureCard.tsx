import { useLingui } from '@lingui/react/macro';
import { useLayoutEffect, useRef, type FocusEvent } from 'react';
import { mediaUrl } from '../../bridge/commands';
import { SpriteIcon } from '../../design/SpriteIcon';
import { Icon } from '../../layout/Icon';
import { useChat } from '../../state/chat';
import type { PicturePart } from '../../state/chatMessages';
import { PICTURE_USD, pictureTitle } from '../../state/chatPictures';
import { UndoChip } from './UndoChip';

/**
 * A picture a reply asked for, as a card: the art on a checkerboard (a sticker's cut-out
 * shows), its name, its size and where it goes, and Use (primary) and Try again.
 * - While it is drawn, it says so with a spinner, and what the drawing costs ("Costs about $0.01."). Try again keeps
 *   the picture drawn before, and Use, while it draws another ("Drawing a new one.").
 * - A drawing that failed says why (an alert), with a still placeholder when there is no picture yet, and Try again.
 * - Use places it as one undo step, and the actions become its Undo chip. Once it can't be placed any more, it says
 *   why, with nothing to press.
 *
 * The keyboard never falls to the page as its parts change: it goes to the Undo chip once placed, to the reason once
 * gone, and to the card while it draws.
 */
export function PictureCard({ messageId, part }: { messageId: string; part: PicturePart }) {
  const { t } = useLingui();
  const { picture, state, drawn, reason, applied, again, unshown } = part;
  const title = pictureTitle(picture);
  const card = useRef<HTMLDivElement>(null);
  const why = useRef<HTMLParagraphElement>(null);
  /** The keyboard is in the card (or was, when the control that had it went away). */
  const inside = useRef(false);
  const drawing = state === 'drawing' || again === true;
  const price = `$${PICTURE_USD.toFixed(2)}`;

  useLayoutEffect(() => {
    if (!inside.current) return;
    const now = document.activeElement;
    if (now !== null && now !== document.body) return;
    const chip = card.current?.querySelector<HTMLElement>('.chip');
    const target =
      state === 'placed' && chip ? chip : state === 'gone' && why.current ? why.current : card.current;
    target?.focus();
  }, [state, again]);

  const onBlur = (e: FocusEvent<HTMLDivElement>) => {
    const box = e.currentTarget;
    const left = e.target;
    if (e.relatedTarget instanceof Node && box.contains(e.relatedTarget)) return;
    // Focus given to something outside the card: no longer ours to look after.
    if (e.relatedTarget instanceof Node) return void (inside.current = false);
    // None given: checked once the event is over (as `useReturnFocus` does). A control still in the page that lost it
    // means the person clicked somewhere that takes none; one that is gone went with the card's change, which the
    // effect above looks after.
    queueMicrotask(() => {
      if (left.isConnected && document.activeElement !== left) inside.current = false;
    });
  };

  const tryAgain = (
    <button
      type="button"
      className="btn btn-secondary btn-sm"
      onClick={() => void useChat.getState().drawPicture(messageId, picture.id)}
    >
      <SpriteIcon name="retry" />
      {t({ id: 'chatPanel.picture.again', message: 'Try again' })}
    </button>
  );
  const size =
    drawn &&
    (picture.kind === 'sticker' && !drawn.note
      ? t({
          id: 'chatPanel.picture.sizeClear',
          message: `${drawn.width} x ${drawn.height}, transparent background.`,
        })
      : t({ id: 'chatPanel.picture.size', message: `${drawn.width} x ${drawn.height}.` }));

  return (
    <div
      ref={card}
      className="card gen"
      role="group"
      aria-label={title}
      aria-busy={drawing || undefined}
      tabIndex={-1}
      onFocus={() => (inside.current = true)}
      onBlur={onBlur}
    >
      <div className="gen-art checker">
        {drawn ? (
          <img src={mediaUrl(drawn.path)} alt="" />
        ) : state === 'drawing' ? (
          <span className="spinner" aria-hidden />
        ) : (
          <Icon name="info" />
        )}
      </div>
      <div className="gen-body">
        <p className="gen-title">{title}</p>
        <p className="gen-meta">
          {size && `${size} `}
          {drawn?.note && `${drawn.note} `}
          {picture.place}
        </p>
        {drawing && (
          <p className="gen-meta gen-drawing">
            {drawn && <span className="spinner" aria-hidden />}
            <span>
              {drawn
                ? t({ id: 'chatPanel.picture.drawingAgain', message: 'Drawing a new one.' })
                : t({ id: 'chatPanel.picture.drawing', message: 'Drawing it now.' })}
            </span>{' '}
            <span>
              {t({
                id: 'chatPanel.picture.price',
                message: `Costs about ${price}.`,
                comment: 'While a picture is drawn: what drawing it costs, e.g. “$0.01”',
              })}
            </span>
          </p>
        )}
        {reason && (
          <p className="gen-meta gen-why" role="alert" ref={why} tabIndex={-1}>
            {reason}
          </p>
        )}
        {(state === 'drawn' || state === 'placing') && (
          <div className="p-actions">
            <button
              type="button"
              className="btn btn-primary btn-sm"
              aria-disabled={state === 'placing' || undefined}
              onClick={() => void useChat.getState().usePicture(messageId, picture.id)}
            >
              {t({ id: 'chatPanel.picture.use', message: 'Use' })}
            </button>
            {state === 'drawn' && !again && tryAgain}
          </div>
        )}
        {state === 'failed' && <div className="p-actions">{tryAgain}</div>}
        {state === 'placed' && applied && <UndoChip undo={applied.undo} revision={applied.revision} />}
        {state === 'placed' && unshown && (
          <p className="gen-meta" role="alert">
            {t({
              id: 'chatPanel.picture.unshown',
              message: 'Placed, but the timeline didn’t update. Reopen the project.',
            })}
          </p>
        )}
      </div>
    </div>
  );
}
