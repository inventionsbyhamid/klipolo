import { t } from '@lingui/core/macro';
import { create } from 'zustand';
import type { AiStatus } from '../bindings/AiStatus';
import type { ChatRequest } from '../bindings/ChatRequest';
import { useAi } from '../ai/aiStatus';
import { isCode } from '../bridge/call';
import { chatBridge } from '../bridge/chat';
import { errorMessage } from '../lib/errors';
import { findItem } from '../lib/timeline';
import { session, staleMoments, withMessages, type ChatStore, type Message } from './chatMessages';
import { pictureActions } from './chatPictures';
import { dropStale, proposalActions, replaceOpen, showOpenProposal } from './chatProposals';
import { usePlayback } from './playback';
import { useProject } from './project';
import { useRange } from './range';
import { useSelection } from './selection';
import { useUi } from './ui';

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
  const project = snapshot?.project;
  const range = useRange.getState().range;
  const selected = useSelection.getState().selectedId;
  const item = project && selected && findItem(project, selected) ? selected : null;
  return {
    id,
    text,
    selection: range ? { start: range.startUs, end: range.endUs } : null,
    item,
    playheadUs: usePlayback.getState().playheadUs,
    format: useUi.getState().previewFormat ?? project?.settings.primaryFormat ?? null,
    keepOnly,
    baseRevision: baseRevision ?? snapshot?.history.revision ?? 0,
  };
}

/**
 * The chat panel's conversation: one message at a time, the replies as Rust sent them (their words
 * clean), the additions merged as one undo step, the proposal cards (`chatProposals`), whose open one the timeline shows
 * red (`useProposal`).
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

    reset: (projectId) => {
      session.next();
      set({ projectId, messages: [], pending: null, draft: '' });
    },

    newChat: () => {
      session.next();
      set({ messages: [], pending: null });
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
        if (!live()) return;
        if (outcome.type === 'failed') {
          // Declined under the provider's policy: the same words would be declined again, so they go back into the
          // box to be reworded (unless the person typed something new meanwhile).
          if (outcome.kind === 'declined' && get().draft.trim() === '') set({ draft: trimmed });
          return failed(outcome.message, outcome.kind);
        }
        // The additions first, as one undo step, then what the model's undo changed: the proposal was made on the
        // project they leave.
        let applied = null;
        if (outcome.applied) {
          await useProject.getState().adopt(outcome.applied.change);
          applied = { undo: outcome.applied.undo, revision: outcome.applied.change.history.revision };
        }
        if (outcome.undone) await useProject.getState().adopt(outcome.undone);
        if (!live()) return;
        const revision = useProject.getState().snapshot?.history.revision;
        const proposal = outcome.proposal && {
          proposal: outcome.proposal,
          state: outcome.proposal.baseRevision === revision ? ('open' as const) : ('dropped' as const),
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
        });
        // Each picture it asked for is drawn now, for its card.
        for (const picture of outcome.pictures) void get().drawPicture(id, picture.id);
      } catch (error) {
        if (!live()) return;
        // Its times were on a revision the project left: sent again it would be refused again, so it says to ask anew.
        if (isCode(error, 'project.moved'))
          push({
            id,
            from: 'klipolo',
            kind: 'failed',
            text: t({ id: 'chatPanel.moved', message: 'The video changed. Ask again.' }),
            failure: 'other',
            retry: null,
          });
        else failed(errorMessage(error), 'other');
      } finally {
        set((s) => (s.pending?.id === id ? { pending: null } : {}));
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
  };
});

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
