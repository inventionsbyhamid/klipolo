import { useLingui } from '@lingui/react/macro';
import { mediaUrl } from '../../bridge/commands';
import { SpriteIcon } from '../../design/SpriteIcon';
import { useChat } from '../../state/chat';
import type { PicturePart } from '../../state/chatMessages';
import { UndoChip } from './UndoChip';

/**
 * A picture a reply asked for, as a card: the art on a checkerboard (a sticker's cut-out
 * shows), its name, what made it, and Use (primary) and Try again. While it is drawn, it says so; a drawing that failed
 * says why, with Try again. Use places it as one undo step and the actions become its Undo chip. Once it can't be
 * placed any more, it says why, with nothing to press.
 */
export function PictureCard({ messageId, part }: { messageId: string; part: PicturePart }) {
  const { t } = useLingui();
  const { picture, state, drawn, reason, applied } = part;
  const name =
    drawn?.name ??
    (picture.kind === 'sticker'
      ? t({ id: 'chatPanel.picture.sticker', message: `${picture.prompt} sticker` })
      : t({ id: 'chatPanel.picture.background', message: `${picture.prompt} title card background` }));
  const title = name.charAt(0).toUpperCase() + name.slice(1);
  const chat = useChat.getState();
  const tryAgain = (
    <button
      type="button"
      className="btn btn-secondary btn-sm"
      onClick={() => void chat.drawPicture(messageId, picture.id)}
    >
      <SpriteIcon name="retry" />
      {t({ id: 'chatPanel.picture.again', message: 'Try again' })}
    </button>
  );
  return (
    <div className="card gen" role="group" aria-label={title} aria-busy={state === 'drawing' || undefined}>
      <div className="gen-art checker">
        {drawn && state !== 'drawing' ? (
          <img src={mediaUrl(drawn.path)} alt={title} />
        ) : (
          <span className="spinner" aria-hidden />
        )}
      </div>
      <div className="gen-body">
        <p className="gen-title">{title}</p>
        {state === 'drawing' ? (
          <p className="gen-meta">{t({ id: 'chatPanel.picture.drawing', message: 'Drawing it now.' })}</p>
        ) : (
          drawn && (
            <p className="gen-meta">
              {t({ id: 'chatPanel.picture.madeWith', message: `Made with ${drawn.madeWith}.` })}
              {drawn.note && ` ${drawn.note}`}
            </p>
          )
        )}
        {reason && <p className="gen-meta">{reason}</p>}
        {(state === 'drawn' || state === 'placing') && (
          <div className="p-actions">
            <button
              type="button"
              className="btn btn-primary btn-sm"
              aria-disabled={state === 'placing' || undefined}
              onClick={() => void chat.usePicture(messageId, picture.id)}
            >
              {t({ id: 'chatPanel.picture.use', message: 'Use' })}
            </button>
            {state === 'drawn' && tryAgain}
          </div>
        )}
        {state === 'failed' && <div className="p-actions">{tryAgain}</div>}
        {state === 'placed' && applied && <UndoChip undo={applied.undo} revision={applied.revision} />}
      </div>
    </div>
  );
}
