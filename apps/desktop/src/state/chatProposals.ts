import type { ChatApplied } from '../bindings/ChatApplied';
import { isCode } from '../bridge/call';
import { chatBridge } from '../bridge/chat';
import { errorMessage } from '../lib/errors';
import {
  openProposal,
  session,
  withMessages,
  type ChatGet,
  type ChatSet,
  type ChatStore,
  type Message,
  type ProposalPart,
  type ReplyMessage,
} from './chatMessages';
import { useProject } from './project';
import { useProposal } from './proposal';

/**
 * The chat store's proposal cards: Apply (one undo step, merged as Rust made it), Discard (nothing
 * changes), and dropping an open one when the project changes under it (it was made on an older revision).
 */
export function proposalActions(set: ChatSet, get: ChatGet): Pick<ChatStore, 'apply' | 'discard'> {
  const update = (id: string, patch: (p: ProposalPart) => Partial<ProposalPart>) =>
    set((s) =>
      withMessages(
        s.messages.map((m) =>
          m.id === id && m.from === 'klipolo' && m.kind === 'reply' && m.proposal
            ? { ...m, proposal: { ...m.proposal, ...patch(m.proposal) } }
            : m,
        ),
      ),
    );
  const find = (id: string) =>
    get().messages.find(
      (m): m is ReplyMessage => m.id === id && m.from === 'klipolo' && m.kind === 'reply' && m.proposal !== null,
    );

  return {
    apply: async (id) => {
      const m = find(id);
      if (!m || m.proposal?.state !== 'open') return;
      const mine = session.now();
      update(id, () => ({ state: 'applying', reason: undefined }));
      let applied: ChatApplied;
      try {
        applied = await chatBridge().apply(m.proposal.proposal.id);
      } catch (error) {
        if (session.now() !== mine) return;
        // The project changed since it was made (`project.moved`): dropped, as a hand edit drops it. Refused for any
        // other reason, it waits again, saying why, to be tried again or discarded.
        if (isCode(error, 'project.moved')) update(id, () => ({ state: 'dropped' }));
        else update(id, () => ({ state: 'open', reason: errorMessage(error) }));
        return;
      }
      if (session.now() !== mine) return;
      // Rust made the change, so the card never opens again. When the page can't take it in (the project couldn't be
      // read again), it says so rather than offer to apply it twice.
      const shown = await useProject
        .getState()
        .adopt(applied.change)
        .catch(() => false);
      if (session.now() !== mine) return;
      if (shown)
        update(id, () => ({ state: 'applied', undo: applied.undo, revision: applied.change.history.revision }));
      else update(id, () => ({ state: 'unshown' }));
    },

    discard: (id) => {
      const m = find(id);
      if (!m || m.proposal?.state !== 'open') return;
      void chatBridge()
        .discard(m.proposal.proposal.id)
        .catch(() => undefined);
      update(id, () => ({ state: 'discarded' }));
    },
  };
}

/**
 * `messages` with every open proposal marked replaced, as a newer one comes (one open proposal per project: red always
 * means "will be cut"), and the ids of those it replaced, for Rust to forget.
 */
export function replaceOpen(messages: Message[]): { messages: Message[]; replaced: string[] } {
  const replaced: string[] = [];
  const next = messages.map((m) => {
    if (m.from !== 'klipolo' || m.kind !== 'reply' || m.proposal?.state !== 'open') return m;
    replaced.push(m.proposal.proposal.id);
    return { ...m, proposal: { ...m.proposal, state: 'replaced' as const } };
  });
  return { messages: replaced.length > 0 ? next : messages, replaced };
}

/** `messages` with every open proposal not made on `revision` dropped (a hand edit, an undo, another project). */
export function dropStale(messages: Message[], revision: number | null): Message[] | null {
  let changed = false;
  const next = messages.map((m) => {
    if (m.from !== 'klipolo' || m.kind !== 'reply' || m.proposal?.state !== 'open') return m;
    if (m.proposal.proposal.baseRevision === revision) return m;
    changed = true;
    return { ...m, proposal: { ...m.proposal, state: 'dropped' as const } };
  });
  return changed ? next : null;
}

/** The timeline shows the open proposal's ranges red (`useProposal`), and nothing once it is applied or gone. */
export function showOpenProposal(messages: Message[]) {
  const open = openProposal(messages)?.part.proposal ?? null;
  const shown = useProposal.getState().proposal;
  if ((shown?.id ?? null) === (open?.id ?? null)) return;
  useProposal.getState().show(
    open && { id: open.id, baseRevision: open.baseRevision, removals: open.removals, tags: open.tags },
  );
}
