import type { ChatApplied } from '../bindings/ChatApplied';
import { chatBridge } from '../bridge/chat';
import { errorMessage } from '../lib/errors';
import {
  session,
  withMessages,
  type ChatGet,
  type ChatSet,
  type ChatStore,
  type PicturePart,
  type ReplyMessage,
} from './chatMessages';
import { useProject } from './project';

/**
 * The chat store's picture cards: each picture a reply asks for is drawn as
 * reply lands; Try again draws it again; Use places it as one undo step, merged as Rust made it, and the card keeps an
 * Undo chip. A picture Rust can't place any more (the moment it was for was cut, its title is gone) says why, with
 * nothing to press: Rust sends that as a sentence, while a coded refusal (the project moved meanwhile) leaves Use there.
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

  return {
    drawPicture: async (messageId, pictureId) => {
      const part = find(messageId, pictureId);
      if (!part || part.state === 'placing' || part.state === 'placed' || part.state === 'gone') return;
      const mine = session.now();
      update(messageId, pictureId, () => ({ state: 'drawing', reason: undefined }));
      try {
        const drawn = await chatBridge().drawPicture(pictureId);
        if (session.now() === mine) update(messageId, pictureId, () => ({ state: 'drawn', drawn }));
      } catch (error) {
        if (session.now() === mine) update(messageId, pictureId, () => ({ state: 'failed', reason: errorMessage(error) }));
      }
    },

    usePicture: async (messageId, pictureId) => {
      const part = find(messageId, pictureId);
      if (part?.state !== 'drawn') return;
      const mine = session.now();
      update(messageId, pictureId, () => ({ state: 'placing', reason: undefined }));
      let applied: ChatApplied;
      try {
        applied = await chatBridge().usePicture(pictureId);
      } catch (error) {
        if (session.now() !== mine) return;
        // A sentence from Rust: it can't be placed any more. Anything else (a coded refusal): Use stays, with why.
        const gone = error instanceof Error && typeof error.cause === 'string';
        update(messageId, pictureId, () => ({ state: gone ? 'gone' : 'drawn', reason: errorMessage(error) }));
        return;
      }
      if (session.now() !== mine) return;
      const shown = await useProject
        .getState()
        .adopt(applied.change)
        .catch(() => false);
      if (session.now() !== mine) return;
      // Placed either way: Rust made the change. When the page couldn't take it in, there is no Undo chip to offer.
      update(messageId, pictureId, () => ({
        state: 'placed',
        applied: shown ? { undo: applied.undo, revision: applied.change.history.revision } : undefined,
      }));
    },
  };
}
