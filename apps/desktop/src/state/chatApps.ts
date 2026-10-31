import { t } from '@lingui/core/macro';
import type { CodedError } from '../bindings/CodedError';
import type { ExternalReply } from '../bindings/ExternalReply';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { inSentence } from '../lib/history';
import { wordLabel, type LabelParams } from '../lib/labels';
import { MCP_LABELS } from '../lib/labelTable';
import {
  openProposal,
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

/**
 * `label` in words without the app's name, which the message's header gives already: "Make it vertical". A label of
 * other apps' own (`mcp.fromEarlier`) keeps it, as its words need it: "Earlier changes from Example Client".
 */
function withoutApp(label: CodedError): string {
  if (Object.hasOwn(MCP_LABELS, label.code)) return wordLabel(label.code, label.params);
  const rest: LabelParams = { ...label.params };
  delete rest.client;
  return wordLabel(label.code, rest);
}

/** The label of the step an app's own undo took back, as its history says after it (the step to redo). */
function undoneLabel(undone: SnapshotPatch | null): CodedError | null {
  const history = undone?.history;
  return history?.redoLabel ? { code: history.redoLabel, params: history.redoParams ?? {} } : null;
}

/** Once a project that is opening or closing has landed (the page's copy is the one Rust has open), or at once. */
function switchLanded(): Promise<void> {
  if (!useProject.getState().switching) return Promise.resolve();
  return new Promise((resolve) => {
    const stop = useProject.subscribe((s) => {
      if (s.switching) return;
      stop();
      resolve();
    });
  });
}

/**
 * Merges an app's `change` unless the copy has it already: a project that opened after the app made it holds it (the
 * same opening, at its revision or a newer one).
 */
async function adoptNew(change: SnapshotPatch) {
  const now = useProject.getState().snapshot;
  if (now && now.history.revision >= change.history.revision) return;
  await useProject.getState().adopt(change);
}

/**
 * What another app did, in one line for its message (its reply has no words of its own): the change it made, in the
 * words its Undo uses ("Make it vertical"), or the step of its own it took back ("Undid make it vertical."). Empty when
 * the message shows it already (a proposal, moments, a picture).
 */
export function appLine(m: ReplyMessage): string {
  if (m.applied) return withoutApp(m.applied.undo);
  if (m.undid) {
    const label = inSentence(withoutApp(m.undid));
    if (label)
      return t({ id: 'chatPanel.app.undid', message: `Undid ${label}.`, comment: '{label} is the change taken back' });
  }
  return '';
}

/**
 * The chat store's messages from other apps (MCP), told by Rust (`mcp-reply`) and shown as Klipolo's replies are, named
 * for the app. Its change is merged as one undo step (the app's, labelled with its name); its proposal waits like
 * chat's, red on the timeline, for the person's Apply; its moments and pictures show as chat's. Its pictures are drawn
 * here (nothing else draws them) and wait for the person's Use, as the app was told: never placed by themselves.
 * Replies are taken one at a time, in the order Rust told them, so a proposal is checked on the project the app's
 * change before it left. One told while a project opens waits for it; nothing shows with no project open, or for another
 * opening.
 */
export function appActions(set: ChatSet, get: ChatGet): Pick<ChatStore, 'receive'> {
  let queue: Promise<void> = Promise.resolve();
  const push = (m: Message) => set((s) => withMessages([...s.messages, m]));
  const take = async (external: ExternalReply) => {
    const { client, reply: r } = external;
    // A project opening or closing: it waits until the page has it, then goes by the opening and the revision.
    await switchLanded();
    if (!useProject.getState().snapshot || !fromThisOpening(external)) return;
    // Told twice: shown (and its pictures drawn) once.
    if (get().messages.some((m) => m.id === r.id)) return;
    const mine = session.now();
    let applied: ReplyMessage['applied'] = null;
    if (r.applied) {
      await adoptNew(r.applied.change);
      applied = { undo: r.applied.undo, revision: r.applied.change.history.revision };
    }
    if (r.undone) await adoptNew(r.undone);
    // New chat or another project meanwhile: the change is merged (Rust made it), but the conversation it was for is
    // gone.
    if (session.now() !== mine) return;
    const revision = useProject.getState().snapshot?.history.revision;
    // Made on the project as it is (or as it is once a change still on its way here lands): it waits. Made on one the
    // project has left: dropped, as Rust drops it.
    let proposal: ReplyMessage['proposal'] = null;
    if (r.proposal) {
      let state: ProposalState = revision !== undefined && r.proposal.baseRevision >= revision ? 'open' : 'dropped';
      // Someone else's proposal waits, made on the same project or a newer one: Klipolo never lets an app's replace it,
      // so this one is older (it was replaced there before its news got here). The one waiting stays as it is.
      const waiting = openProposal(get().messages);
      const older = waiting && waiting.message.client !== client;
      if (state === 'open' && older && waiting.part.proposal.baseRevision >= r.proposal.baseRevision)
        state = 'superseded';
      proposal = { proposal: r.proposal, state };
    }
    // One open proposal at a time: a newer one replaces the one waiting on the page. Rust has replaced it already (its
    // own, or one on a project that has moved on), so nothing is discarded there: a discard could only reach a newer one.
    if (proposal?.state === 'open') {
      const { messages, replaced } = replaceOpen(get().messages);
      set(withMessages(messages));
      if (replaced.length > 0) proposal.swappedAt = performance.now();
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
    // Drawn here (nothing else draws them); placed only by the person's Use, as their own step.
    for (const picture of r.pictures) void get().drawPicture(r.id, picture.id);
  };

  return {
    receive: (external) => {
      queue = queue.then(() => take(external)).catch(() => undefined);
      return queue;
    },
  };
}
