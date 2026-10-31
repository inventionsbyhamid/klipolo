import { t } from '@lingui/core/macro';
import type { ChatApplied } from '../bindings/ChatApplied';
import type { ChatPicture } from '../bindings/ChatPicture';
import { chatBridge } from '../bridge/chat';
import { errorMessage } from '../lib/errors';
import {
  creditMoved,
  session,
  withMessages,
  type ChatGet,
  type ChatSet,
  type ChatStore,
  type PicturePart,
  type ReplyMessage,
} from './chatMessages';
import { useProject } from './project';

/** The pictures being drawn now, by id: a picture is never drawn twice at once (each drawing is paid for). */
const drawing = new Set<string>();

/** A picture's name, the same before and after it is drawn: "Paper plane sticker". */
export function pictureTitle(picture: ChatPicture): string {
  const name =
    picture.kind === 'sticker'
      ? t({ id: 'chatPanel.picture.sticker', message: `${picture.prompt} sticker` })
      : t({ id: 'chatPanel.picture.background', message: `${picture.prompt} title card background` });
  return name.charAt(0).toUpperCase() + name.slice(1);
}

/**
 * The chat store's picture cards: each picture a reply asks for is drawn as
 * the reply lands. Try again draws another while the one drawn before stays usable; a Try again that fails leaves it
 * so, saying why. Use places it in the commit queue's turn (`useProject.stepWith`) as one undo step, merged as Rust
 * made it, and the card keeps an Undo chip. A picture Rust can't place any more (the moment it was for was cut, its
 * title is gone) says why with nothing to press: Rust sends that as a sentence, while a coded refusal leaves Use
 * there. A result that comes after New chat still changes the project when Rust changed it; only the card is gone.
 */
export function pictureActions(set: ChatSet, get: ChatGet): Pick<ChatStore, 'drawPicture' | 'usePicture'> {
  const update = (messageId: string, pictureId: string, patch: (p: PicturePart) => Partial<PicturePart>) =>
    set((s) =>
      withMessages(
        s.messages.map((m) =>
          m.id === messageId && m.from === 'klipolo' && m.kind === 'reply'
            ? {
                ...m,
                pictures: m.pictures.map((p) => (p.picture.id === pictureId ? { ...p, ...patch(p) } : p)),
              }
            : m,
        ),
      ),
    );
  const find = (messageId: string, pictureId: string) =>
    get()
      .messages.find((m): m is ReplyMessage => m.id === messageId && m.from === 'klipolo' && m.kind === 'reply')
      ?.pictures.find((p) => p.picture.id === pictureId);
  /** Placing it, placed or gone: a drawing's result no longer changes the card. */
  const settled = (p: PicturePart | undefined) =>
    p === undefined || p.state === 'placing' || p.state === 'placed' || p.state === 'gone';

  return {
    drawPicture: async (messageId, pictureId) => {
      const part = find(messageId, pictureId);
      if (settled(part) || drawing.has(pictureId)) return;
      const mine = session.now();
      drawing.add(pictureId);
      update(messageId, pictureId, (p) =>
        p.drawn ? { again: true, reason: undefined } : { state: 'drawing', reason: undefined },
      );
      try {
        const drawn = await chatBridge().drawPicture(pictureId);
        if (session.now() !== mine || settled(find(messageId, pictureId))) return;
        update(messageId, pictureId, () => ({ state: 'drawn', drawn, again: false, reason: undefined }));
        get().say(t({ id: 'chatPanel.picture.ready', message: `${pictureTitle(part!.picture)} is ready.` }));
      } catch (error) {
        if (session.now() !== mine || settled(find(messageId, pictureId))) return;
        // The picture drawn before stays usable; with none, the card says why and offers Try again.
        update(messageId, pictureId, (p) =>
          p.drawn
            ? { state: 'drawn', again: false, reason: errorMessage(error) }
            : { state: 'failed', again: false, reason: errorMessage(error) },
        );
      } finally {
        drawing.delete(pictureId);
        creditMoved();
      }
    },

    usePicture: async (messageId, pictureId) => {
      const part = find(messageId, pictureId);
      if (part?.state !== 'drawn') return;
      const mine = session.now();
      update(messageId, pictureId, () => ({ state: 'placing', reason: undefined }));
      let applied: ChatApplied | undefined;
      let shown: boolean;
      try {
        // In the commit queue's turn, behind a change still settling: merged whatever became of the conversation.
        shown = await useProject.getState().stepWith(() =>
          chatBridge()
            .usePicture(pictureId)
            .then((made) => {
              applied = made;
              return made.change;
            }),
        );
      } catch (error) {
        if (session.now() !== mine) return;
        // A sentence from Rust: it can't be placed any more. Anything else (a coded refusal): Use stays, with why.
        const gone = error instanceof Error && typeof error.cause === 'string';
        update(messageId, pictureId, () => ({ state: gone ? 'gone' : 'drawn', reason: errorMessage(error) }));
        return;
      }
      if (session.now() !== mine) return;
      if (!applied) {
        // Not sent (the project can't be edited now): Use stays.
        update(messageId, pictureId, () => ({ state: 'drawn' }));
        return;
      }
      const made = applied;
      // Placed either way: Rust made the change. When the page couldn't take it in, it says so, with no Undo chip.
      update(messageId, pictureId, () =>
        shown
          ? { state: 'placed', applied: { undo: made.undo, revision: made.change.history.revision } }
          : { state: 'placed', unshown: true },
      );
      if (shown) get().say(t({ id: 'chatPanel.picture.placed', message: 'Placed.' }));
    },
  };
}
