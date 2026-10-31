import { t } from '@lingui/core/macro';
import type { CodedError } from '../bindings/CodedError';
import type { ExternalReply } from '../bindings/ExternalReply';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { chatBridge } from '../bridge/chat';
import { inSentence } from '../lib/history';
import { wordLabel, type LabelParams } from '../lib/labels';
import {
  session,
  withMessages,
  type ChatGet,
  type ChatSet,
  type ChatStore,
  type Message,
  type ProposalState,
  type ReplyMessage,
} from './chatMessages';
import { replaceOpen } from './chatProposals';
import { useProject } from './project';

/**
 * Whether an app's reply is about the project as it is open now. Rust names the opening an app's change was made on
 * (revisions start again at 0 on each opening, so a revision alone can't tell two openings apart): a reply about
 * another opening is ignored. A copy that doesn't know its opening takes it as its own.
 */
export function fromThisOpening(external: ExternalReply): boolean {
  const open = useProject.getState().snapshot?.opening;
  return open === undefined || external.opening === open;
}

/** `params` without the app's name: the message's header names the app already. */
function withoutClient(params: LabelParams): LabelParams {
  const rest = { ...params };
  delete rest.client;
  return rest;
}

/** The label of the step an app's own undo took back, as its history says after it (the step to redo). */
function undoneLabel(undone: SnapshotPatch | null): CodedError | null {
  const history = undone?.history;
  return history?.redoLabel ? { code: history.redoLabel, params: history.redoParams ?? {} } : null;
}

/**
 * What another app did, in one line for its message (its reply has no words of its own): the change it made, in the
 * words its Undo uses ("Make it vertical"), or the step of its own it took back ("Undid make it vertical."). Empty when
 * the message shows it already (a proposal, moments, a picture).
 */
export function appLine(m: ReplyMessage): string {
  if (m.applied) return wordLabel(m.applied.undo.code, withoutClient(m.applied.undo.params));
  if (m.undid) {
    const label = inSentence(wordLabel(m.undid.code, withoutClient(m.undid.params)));
    if (label)
      return t({ id: 'mcp.undid', message: `Undid ${label}.`, comment: '{label} is the change taken back' });
  }
  return '';
}

/**
 * The chat store's messages from other apps (MCP), told by Rust (`mcp-reply`) and shown as Klipolo's replies are, named
 * for the app. Its change is merged as one undo step (the app's, labelled with its name); its proposal waits like
 * chat's, red on the timeline, for the person's Apply; its moments and pictures show as chat's. Its pictures are drawn
 * here (nothing else draws them) and placed once drawn, as the app was told they would be: the card keeps its Undo chip.
 * Replies are taken one at a time, in the order Rust told them, so a proposal is checked on the project the app's
 * change before it left. Nothing shows while no project is open or one is opening or closing, or for another opening.
 */
export function appActions(set: ChatSet, get: ChatGet): Pick<ChatStore, 'receive'> {
  let queue: Promise<void> = Promise.resolve();
  const push = (m: Message) => set((s) => withMessages([...s.messages, m]));
  const part = (messageId: string, pictureId: string) =>
    get()
      .messages.find((m): m is ReplyMessage => m.id === messageId && m.from === 'klipolo' && m.kind === 'reply')
      ?.pictures.find((p) => p.picture.id === pictureId);

  const drawThenPlace = async (messageId: string, pictureId: string) => {
    await get().drawPicture(messageId, pictureId);
    const p = part(messageId, pictureId);
    // Drawn, and not placed or drawing again yet: placed once, as the person's Use would.
    if (p?.state === 'drawn' && p.drawn && !p.again && !p.reason) await get().usePicture(messageId, pictureId);
  };

  const take = async (external: ExternalReply) => {
    const { client, reply: r } = external;
    const project = useProject.getState();
    if (!project.snapshot || project.switching || !fromThisOpening(external)) return;
    const mine = session.now();
    let applied: ReplyMessage['applied'] = null;
    if (r.applied) {
      await useProject.getState().adopt(r.applied.change);
      applied = { undo: r.applied.undo, revision: r.applied.change.history.revision };
    }
    if (r.undone) await useProject.getState().adopt(r.undone);
    // New chat or another project meanwhile: the change is merged (Rust made it), but the conversation it was for is
    // gone.
    if (session.now() !== mine) return;
    const revision = useProject.getState().snapshot?.history.revision;
    // Made on the project as it is (or as it is once a change still on its way here lands): it waits. Made on one the
    // project has left: dropped, as Rust drops it.
    let proposal: ReplyMessage['proposal'] = null;
    if (r.proposal) {
      const open = revision !== undefined && r.proposal.baseRevision >= revision;
      proposal = { proposal: r.proposal, state: (open ? 'open' : 'dropped') as ProposalState };
    }
    // One open proposal at a time: a newer one replaces the one waiting (Rust has already).
    if (proposal?.state === 'open') {
      const { messages, replaced } = replaceOpen(get().messages);
      set(withMessages(messages));
      for (const old of replaced)
        void chatBridge()
          .discard(old)
          .catch(() => undefined);
    }
    push({
      id: r.id,
      from: 'klipolo',
      kind: 'reply',
      client,
      text: r.text,
      notes: r.notes,
      questions: r.questions,
      moments: r.moments,
      revision: r.revision,
      momentsStale: r.moments.length > 0 && r.revision !== revision,
      applied,
      undid: undoneLabel(r.undone),
      proposal,
      pictures: r.pictures.map((picture) => ({ picture, state: 'drawing' as const })),
    });
    for (const picture of r.pictures) void drawThenPlace(r.id, picture.id);
  };

  return {
    receive: (external) => {
      queue = queue.then(() => take(external)).catch(() => undefined);
      return queue;
    },
  };
}
