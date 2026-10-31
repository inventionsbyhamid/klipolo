import { create } from 'zustand';
import type { AiStatus } from '../bindings/AiStatus';
import type { ChatRequest } from '../bindings/ChatRequest';
import { useAi } from '../ai/aiStatus';
import { chatBridge } from '../bridge/chat';
import { errorMessage } from '../lib/errors';
import { findItem } from '../lib/timeline';
import { session, withMessages, type ChatStore, type Message } from './chatMessages';
import { dropStale, proposalActions, showOpenProposal } from './chatProposals';
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

/** What "this" and "here" mean as a message goes: the time range or the overlay selected, the playhead, the format. */
function requestFor(id: string, text: string, keepOnly: { start: number; end: number }[]): ChatRequest {
  const project = useProject.getState().snapshot?.project;
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
  };
}

/**
 * The chat panel's conversation: one message at a time, the replies as Rust sent them (their words
 * clean), the additions merged as one undo step, the proposal cards (`chatProposals`), whose open one the timeline shows
 * red (`useProposal`).
 */
export const useChat = create<ChatStore>((set, get) => {
  const push = (m: Message) => set((s) => withMessages([...s.messages, m]));

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
      if (!snapshot || !trimmed || get().pending || snapshot.readOnly != null) return;
      if (!chatAllowed(useAi.getState().status)) return;
      const mine = session.now();
      const live = () => session.now() === mine && get().projectId === snapshot.project.id;
      const id = crypto.randomUUID();
      const keepOnly = options?.keepOnly ?? [];
      set({ pending: { id } });
      if (options?.echo !== false) push({ id: `${id}:you`, from: 'person', text: trimmed });
      const failed = (message: string, failure: Extract<Message, { kind: 'failed' }>['failure']) =>
        push({ id, from: 'klipolo', kind: 'failed', text: message, failure, retry: { text: trimmed, keepOnly } });
      try {
        const outcome = await chatBridge().send(requestFor(id, trimmed, keepOnly));
        if (!live()) return;
        if (outcome.type === 'failed') return failed(outcome.message, outcome.kind);
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
        push({
          id,
          from: 'klipolo',
          kind: 'reply',
          text: outcome.text,
          notes: outcome.notes,
          questions: outcome.questions,
          moments: outcome.moments,
          applied,
          proposal,
        });
      } catch (error) {
        if (live()) failed(errorMessage(error), 'other');
      } finally {
        set((s) => (s.pending?.id === id ? { pending: null } : {}));
      }
    },

    retry: async (messageId) => {
      const m = get().messages.find((x) => x.id === messageId);
      if (!m || m.from !== 'klipolo' || m.kind !== 'failed' || get().pending) return;
      set((s) => withMessages(s.messages.filter((x) => x !== m)));
      await get().send(m.retry.text, { keepOnly: m.retry.keepOnly, echo: false });
    },

    ...proposalActions(set, get),
  };
});

// The open proposal is the timeline's red ranges.
useChat.subscribe((s, was) => {
  if (s.messages !== was.messages) showOpenProposal(s.messages);
});

// Another project (or the same one opened again, or none) starts the conversation over. A change to the project drops
// an open proposal made on the revision before (a hand edit, an undo): it would cut the wrong footage.
useProject.subscribe((s, was) => {
  const [a, b] = [s.snapshot, was.snapshot];
  if (a?.project.id !== b?.project.id || a?.opening !== b?.opening) {
    useChat.getState().reset(a?.project.id ?? null);
    return;
  }
  if (a && a.history.revision !== b?.history.revision) {
    const next = dropStale(useChat.getState().messages, a.history.revision);
    if (next) useChat.setState(withMessages(next));
  }
});
