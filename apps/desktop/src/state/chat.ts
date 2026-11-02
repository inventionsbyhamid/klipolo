import { t } from '@lingui/core/macro';
import { create } from 'zustand';
import type { AiStatus } from '../bindings/AiStatus';
import type { ChatRequest } from '../bindings/ChatRequest';
import { creditMoved, useAi } from '../ai/aiStatus';
import { isCode } from '../bridge/call';
import { chatBridge } from '../bridge/chat';
import { onMcpReply, type UnlistenFn } from '../bridge/mcp';
import { errorMessage } from '../lib/errors';
import {
  session,
  staleMoments,
  withMessages,
  type ChatStore,
  type Message,
} from './chatMessages';
import { appActions } from './chatApps';
import { pictureActions } from './chatPictures';
import { dropStale, proposalActions, replaceOpen, showOpenProposal } from './chatProposals';
import { currentView } from './editorView';
import { useProject } from './project';
import { useReversing } from './reversing';
import { useSpeech } from './speech';

export { MAX_MESSAGES, type Message, type ReplyMessage, type FailedMessage } from './chatMessages';

/**
 * Whether chat can run on what the AI status says: a key, or an access code the relay hasn't turned off. Unknown
 * (before the first answer) or none: no.
 */
export function chatAllowed(status: AiStatus | null): boolean {
  if (!status) return false;
  return status.mode === 'key' || (status.mode === 'relay' && status.active !== false);
}

/**
 * Development builds only (`dev/devChat`): the stand-in chat is on, which needs no key or code. The AI status stays
 * what Rust said.
 */
export const useChatStandIn = create<{ on: boolean }>(() => ({ on: false }));

/** Whether chat can run now: the AI status allows it, or the development stand-in is on. */
export const canChat = (): boolean => useChatStandIn.getState().on || chatAllowed(useAi.getState().status);

/** `canChat`, followed. */
export function useCanChat(): boolean {
  const standIn = useChatStandIn((s) => s.on);
  const allowed = useAi((s) => chatAllowed(s.status));
  return standIn || allowed;
}

/**
 * What "this" and "here" mean as a message goes: the time range or the overlay selected, the playhead, the format,
 * and the revision those times are on (Rust refuses times on a project that moved on since): `baseRevision` when
 * given (a Retry sends the one it was first sent on), else the one now.
 */
function requestFor(
  id: string,
  text: string,
  keepOnly: { start: number; end: number }[],
  baseRevision?: number,
): ChatRequest {
  const snapshot = useProject.getState().snapshot;
  return {
    id,
    text,
    ...currentView(),
    keepOnly,
    baseRevision: baseRevision ?? snapshot?.history.revision ?? 0,
  };
}

/** Each saying of a notice, so the same words said again are said again. */
let sayings = 0;

/**
 * The chat panel's conversation: one message at a time, the replies as Rust sent them (their words
 * already clean), the additions merged as one undo step, the proposal cards (`chatProposals`), whose open one the
 * timeline shows red (`useProposal`), the picture cards (`chatPictures`), and what other apps did (`chatApps`).
 */
export const useChat = create<ChatStore>((set, get) => {
  const push = (m: Message) => set((s) => withMessages([...s.messages, m]));
  /** A message can go now: a project open and editable, chat allowed, and none under way. */
  const ready = () => {
    const snapshot = useProject.getState().snapshot;
    return snapshot !== null && snapshot.readOnly == null && canChat() && get().pending === null;
  };

  return {
    projectId: null,
    messages: [],
    pending: null,
    draft: '',
    setDraft: (draft) => set({ draft }),
    notice: null,
    say: (text) => set({ notice: { text, seq: ++sayings } }),

    reset: (projectId) => {
      session.next();
      set({ projectId, messages: [], pending: null, draft: '', notice: null });
    },

    newChat: () => {
      session.next();
      set({ messages: [], pending: null, notice: null });
      void chatBridge()
        .reset()
        .catch(() => undefined);
    },

    send: async (text, options) => {
      const snapshot = useProject.getState().snapshot;
      const trimmed = text.trim();
      if (!snapshot || !trimmed || !ready()) return;
      const mine = session.now();
      const live = () => session.now() === mine && get().projectId === snapshot.project.id;
      /** The project it was sent to is still open (New chat or not): Rust's changes to it are the page's too. */
      const sameProject = () => {
        const now = useProject.getState().snapshot;
        return now?.project.id === snapshot.project.id && now.opening === snapshot.opening;
      };
      const id = crypto.randomUUID();
      const keepOnly = options?.keepOnly ?? [];
      set({ pending: { id } });
      if (options?.echo !== false) push({ id: `${id}:you`, from: 'person', text: trimmed });
      const request = requestFor(id, trimmed, keepOnly, options?.baseRevision);
      const again = { text: trimmed, keepOnly, baseRevision: request.baseRevision };
      const failed = (message: string, failure: Extract<Message, { kind: 'failed' }>['failure']) =>
        push({ id, from: 'klipolo', kind: 'failed', text: message, failure, retry: again });
      try {
        const outcome = await chatBridge().send(request);
        // The additions first, as one undo step, then what the model's undo changed: the proposal was made on the
        // project they leave. Merged whenever the project it was sent to is open, even after New chat: Rust made them.
        let applied = null;
        if (outcome.type === 'reply' && sameProject()) {
          if (outcome.applied) {
            await useProject.getState().adopt(outcome.applied.change);
            applied = {
              undo: outcome.applied.undo,
              revision: outcome.applied.change.history.revision,
              changes: outcome.applied.changes,
            };
          }
          if (outcome.undone) await useProject.getState().adopt(outcome.undone);
        }
        if (!live()) return;
        if (outcome.type === 'failed') {
          // Declined under the provider's policy: the same words would be declined again, so they go back into the
          // box to be reworded (unless the person typed something new meanwhile).
          if (outcome.kind === 'declined' && get().draft.trim() === '') set({ draft: trimmed });
          return failed(outcome.message, outcome.kind);
        }
        const revision = useProject.getState().snapshot?.history.revision;
        // Made on the project as it is, or on a change still on its way here (another app's): it waits, and the drop on
        // a revision change checks it again once that lands. Made on one the project has left: dropped.
        const waits = revision !== undefined && outcome.proposal !== null && outcome.proposal.baseRevision >= revision;
        const proposal = outcome.proposal && {
          proposal: outcome.proposal,
          state: waits ? ('open' as const) : ('dropped' as const),
        };
        // One open proposal per project: a newer one replaces the one waiting, which Rust forgets.
        if (proposal?.state === 'open') {
          const { messages, replaced } = replaceOpen(get().messages);
          set(withMessages(messages));
          for (const old of replaced)
            void chatBridge()
              .discard(old)
              .catch(() => undefined);
        }
        push({
          id,
          from: 'klipolo',
          kind: 'reply',
          text: outcome.text,
          notes: outcome.notes,
          questions: outcome.questions,
          moments: outcome.moments,
          revision: outcome.revision,
          // Moments found on another revision than the page has are stale as they come.
          momentsStale: outcome.moments.length > 0 && outcome.revision !== revision,
          applied,
          proposal,
          pictures: outcome.pictures.map((picture) => ({ picture, state: 'drawing' as const })),
          analyze: outcome.analyze,
        });
        // Each picture it asked for is drawn now, for its card.
        for (const picture of outcome.pictures) void get().drawPicture(id, picture.id);
        // Each clip it plays backward is written backward now, as the clip menu's Reverse does.
        for (const clipId of outcome.reverse) useReversing.getState().backward(clipId);
      } catch (error) {
        if (!live()) return;
        // Its times were on a revision the project left: sent again it would be refused again, so it says to ask anew.
        // A message the person typed goes back into the box to send again (unless they typed something new meanwhile);
        // Keep only's words are a label, not theirs.
        if (isCode(error, 'project.moved')) {
          if (keepOnly.length === 0 && get().draft.trim() === '') set({ draft: trimmed });
          push({
            id,
            from: 'klipolo',
            kind: 'failed',
            text: t({ id: 'chatPanel.moved', message: 'The video changed. Ask again.' }),
            failure: 'other',
            retry: null,
          });
        } else failed(errorMessage(error), 'other');
      } finally {
        set((s) => (s.pending?.id === id ? { pending: null } : {}));
        creditMoved();
      }
    },

    retry: async (messageId) => {
      const m = get().messages.find((x) => x.id === messageId);
      // Checked before the failure goes: a Retry that can't send leaves it as it was.
      if (!m || m.from !== 'klipolo' || m.kind !== 'failed' || !m.retry || !ready()) return;
      const { text, keepOnly, baseRevision } = m.retry;
      set((s) => withMessages(s.messages.filter((x) => x !== m)));
      await get().send(text, { keepOnly, echo: false, baseRevision });
    },

    ...proposalActions(set, get),
    ...pictureActions(set, get),
    ...appActions(set, get),
  };
});

/** What other apps do (`mcp-reply`) comes into the conversation, named for each app (`chatApps`). */
export function followApps(): Promise<UnlistenFn> {
  return onMcpReply((external) => void useChat.getState().receive(external));
}

// The open proposal is the timeline's red ranges.
useChat.subscribe((s, was) => {
  if (s.messages !== was.messages) showOpenProposal(s.messages);
});

// Another project (or the same one opened again, or none) starts the conversation over. A change to the project drops
// an open proposal made on the revision before (a hand edit, an undo): it would cut the wrong footage. It also leaves
// the moments replies gave before it with times that may have moved: they can't be kept only any more.
useProject.subscribe((s, was) => {
  const [a, b] = [s.snapshot, was.snapshot];
  if (a?.project.id !== b?.project.id || a?.opening !== b?.opening) {
    useChat.getState().reset(a?.project.id ?? null);
    return;
  }
  if (a && a.history.revision !== b?.history.revision) {
    const was = useChat.getState().messages;
    const dropped = dropStale(was, a.history.revision) ?? was;
    const next = staleMoments(dropped, a.history.revision) ?? dropped;
    if (next !== was) useChat.setState(withMessages(next));
  }
});

// Words timed again (Time words again above the video, or word timing finishing) are new words at new times on the same
// revision: an open proposal whose cuts stand on them would cut where they were, so it is dropped as a hand edit drops
// it (Rust refuses it too). Words of a file it doesn't cut into leave it open. The panel follows `speech-changed`
// (`startSpeechFeed`).
useSpeech.subscribe((s, was) => {
  if (s.pagesEpoch === was.pagesEpoch) return;
  const next = dropStale(useChat.getState().messages, s.wordsChanged);
  if (next) useChat.setState(withMessages(next));
});
