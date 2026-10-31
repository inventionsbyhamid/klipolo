import type { ChatApplied } from '../bindings/ChatApplied';
import type { ChatOutcome } from '../bindings/ChatOutcome';
import type { ChatProposal } from '../bindings/ChatProposal';
import type { ChatReply } from '../bindings/ChatReply';
import type { ChatRequest } from '../bindings/ChatRequest';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { useProject } from '../state/project';
import type { ChatBridge } from './chat';
import { bridge } from './commands';

/**
 * How a fake chat answers: `reply` for every message; `apply` for Apply (else refused, as for a gone proposal);
 * `undoLast` for the Undo chip (else the project's own undo while that change is still its latest, as Rust allows).
 */
export interface FakeChatScript {
  reply: (request: ChatRequest) => ChatOutcome | Promise<ChatOutcome>;
  apply?: (proposalId: string) => ChatApplied | Promise<ChatApplied>;
  undoLast?: (revision: number) => SnapshotPatch | Promise<SnapshotPatch>;
}

/** A fake chat bridge and what it was asked. */
export type FakeChat = ChatBridge & {
  requests: ChatRequest[];
  applied: string[];
  discarded: string[];
  resets: number;
  /** The revisions the Undo chip asked to take back. */
  undone: number[];
};

/** A chat bridge that answers from `script` (tests; a development build's fake chat). */
export function fakeChat(script: FakeChatScript): FakeChat {
  const fake: FakeChat = {
    requests: [],
    applied: [],
    discarded: [],
    resets: 0,
    undone: [],
    send: async (request) => {
      fake.requests.push(request);
      const outcome = await script.reply(request);
      // A reply that doesn't say its revision gets the one Rust would: after what it applied, else the request's.
      if (outcome.type !== 'reply' || outcome.revision >= 0) return outcome;
      const after = outcome.undone ?? outcome.applied?.change;
      return { ...outcome, revision: after?.history.revision ?? request.baseRevision };
    },
    apply: async (proposalId) => {
      fake.applied.push(proposalId);
      if (!script.apply) throw new Error('This proposal is gone. Ask again.');
      return script.apply(proposalId);
    },
    discard: async (proposalId) => {
      fake.discarded.push(proposalId);
    },
    reset: async () => {
      fake.resets += 1;
    },
    undoLast: async (revision) => {
      fake.undone.push(revision);
      if (script.undoLast) return script.undoLast(revision);
      if (useProject.getState().snapshot?.history.revision !== revision)
        throw new Error('That change is no longer the last one.', { cause: { code: 'project.moved', params: {} } });
      return bridge.undo();
    },
  };
  return fake;
}

/**
 * A reply as Rust builds it (its text already clean), with `parts` over an empty one. Its `revision` is -1 unless
 * given: `fakeChat` fills it in as Rust would.
 */
export function reply(id: string, text: string, parts: Partial<ChatReply> = {}): ChatOutcome {
  return {
    type: 'reply',
    id,
    text,
    notes: [],
    questions: [],
    applied: null,
    undone: null,
    proposal: null,
    moments: [],
    revision: -1,
    pictures: [],
    ...parts,
  };
}

/** A proposal as Rust builds it, with `parts` over an empty one made on `baseRevision`. */
export function proposal(id: string, baseRevision: number, parts: Partial<ChatProposal> = {}): ChatProposal {
  return {
    id,
    baseRevision,
    title: '',
    removals: {
      ranges: [],
      clips: [],
      items: [],
      shortened: [],
      tracks: [],
      effects: [],
      transitions: [],
      assets: [],
      luts: [],
      recordings: [],
      markers: [],
      sounds: [],
      formats: [],
    },
    removedUs: 0,
    tags: [],
    ...parts,
  };
}
