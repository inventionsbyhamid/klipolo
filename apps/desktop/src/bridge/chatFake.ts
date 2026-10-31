import type { ChatApplied } from '../bindings/ChatApplied';
import type { ChatOutcome } from '../bindings/ChatOutcome';
import type { ChatProposal } from '../bindings/ChatProposal';
import type { ChatReply } from '../bindings/ChatReply';
import type { ChatRequest } from '../bindings/ChatRequest';
import type { ChatBridge } from './chat';

/** How a fake chat answers: `reply` for every message; `apply` for Apply (else refused, as for a gone proposal). */
export interface FakeChatScript {
  reply: (request: ChatRequest) => ChatOutcome | Promise<ChatOutcome>;
  apply?: (proposalId: string) => ChatApplied | Promise<ChatApplied>;
}

/** A fake chat bridge and what it was asked. */
export type FakeChat = ChatBridge & {
  requests: ChatRequest[];
  applied: string[];
  discarded: string[];
  resets: number;
};

/** A chat bridge that answers from `script` (tests; a development build's fake chat). */
export function fakeChat(script: FakeChatScript): FakeChat {
  const fake: FakeChat = {
    requests: [],
    applied: [],
    discarded: [],
    resets: 0,
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
