import type { StoreApi } from 'zustand';
import type { ChatFailureKind } from '../bindings/ChatFailureKind';
import type { ChatMoment } from '../bindings/ChatMoment';
import type { ChatProposal } from '../bindings/ChatProposal';
import type { CodedError } from '../bindings/CodedError';

/**
 * Where a proposal stands: waiting, being applied, applied, discarded, dropped (the project changed), replaced (a newer
 * one came: a project has one open proposal at a time).
 */
export type ProposalState = 'open' | 'applying' | 'applied' | 'discarded' | 'dropped' | 'replaced';

/** A proposal in a reply, and what came of it. */
export interface ProposalPart {
  proposal: ChatProposal;
  state: ProposalState;
  /** Applied: its undo label, and the revision Apply made (its Undo chip works while that is the latest change). */
  undo?: CodedError;
  revision?: number;
  /** Open again after Apply was refused (not because the project moved): why, in one sentence, with Retry. */
  reason?: string;
}

/**
 * The conversation as the panel shows it. Klipolo's words are kept as Rust sent them: already plain sentences without
 * em or en dashes (`klipolo_app::chat`), never cleaned here.
 */
export type Message =
  | { id: string; from: 'person'; text: string }
  | {
      id: string;
      from: 'klipolo';
      kind: 'reply';
      text: string;
      /** What couldn't be done, one sentence each. */
      notes: string[];
      questions: string[];
      moments: ChatMoment[];
      /** The revision the reply landed on (its additions in): the one its moments' times are on. */
      revision: number | null;
      /** The project changed since it landed: its moments still seek, but can't be kept only (their times moved). */
      momentsStale: boolean;
      /** Additions applied at once: their undo label and the revision they made (the Undo chip). */
      applied: { undo: CodedError; revision: number } | null;
      proposal: ProposalPart | null;
    }
  | {
      id: string;
      from: 'klipolo';
      kind: 'failed';
      text: string;
      failure: ChatFailureKind;
      /**
       * What Retry sends again, on the revision it was first sent on (its selection's and moments' times are on it);
       * null when sending it again can't help (it was refused because the project moved).
       */
      retry: { text: string; keepOnly: { start: number; end: number }[]; baseRevision: number } | null;
    };

export type ReplyMessage = Extract<Message, { kind: 'reply' }>;
export type FailedMessage = Extract<Message, { kind: 'failed' }>;

/** A message under way. */
export interface Pending {
  id: string;
}

/** The conversation keeps this many messages; older ones are dropped, so a long session stays small. */
export const MAX_MESSAGES = 200;

export interface ChatStore {
  projectId: string | null;
  messages: Message[];
  pending: Pending | null;
  /** What the person has typed in the chat box and not sent: kept while chat is folded. */
  draft: string;
  setDraft: (draft: string) => void;
  /** Another project (or none), or New chat: the conversation starts over, and a reply under way is dropped. */
  reset: (projectId: string | null) => void;
  /**
   * The person's message ("this" is the selection). `keepOnly`: "Keep only these" under moments. `baseRevision`: the
   * revision its times are on, when not the one now (a Retry).
   */
  send: (
    text: string,
    options?: { keepOnly?: { start: number; end: number }[]; echo?: boolean; baseRevision?: number },
  ) => Promise<void>;
  /** Sends a failed message again, in its place. */
  retry: (messageId: string) => Promise<void>;
  /** New chat: Rust forgets the conversation too. */
  newChat: () => void;
  apply: (messageId: string) => Promise<void>;
  discard: (messageId: string) => void;
}

export type ChatSet = StoreApi<ChatStore>['setState'];
export type ChatGet = StoreApi<ChatStore>['getState'];

/** `messages` with the oldest dropped past the cap. */
export function withMessages(messages: Message[]) {
  return { messages: messages.length > MAX_MESSAGES ? messages.slice(messages.length - MAX_MESSAGES) : messages };
}

/** The newest proposal still waiting for the person, and its message: what the timeline shows red. */
export function openProposal(messages: Message[]): { message: ReplyMessage; part: ProposalPart } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.from === 'klipolo' && m.kind === 'reply' && m.proposal?.state === 'open') return { message: m, part: m.proposal };
  }
  return null;
}

/**
 * `messages` with the moments of every reply that landed on another revision than `revision` marked stale, for good (an
 * undo back to it is a change too); null when nothing changed.
 */
export function staleMoments(messages: Message[], revision: number): Message[] | null {
  let changed = false;
  const next = messages.map((m) => {
    if (m.from !== 'klipolo' || m.kind !== 'reply' || m.moments.length === 0 || m.momentsStale) return m;
    if (m.revision === revision) return m;
    changed = true;
    return { ...m, momentsStale: true };
  });
  return changed ? next : null;
}

/** Each reset starts a new session: a reply or an apply that resolves after it belongs to the one before. */
let current = 0;
export const session = {
  now: () => current,
  next: () => {
    current += 1;
  },
};
