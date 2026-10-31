import type { StoreApi } from 'zustand';
import { refreshAiStatus, useAi } from '../ai/aiStatus';
import type { ChatFailureKind } from '../bindings/ChatFailureKind';
import type { ChatMoment } from '../bindings/ChatMoment';
import type { ChatPicture } from '../bindings/ChatPicture';
import type { ChatPictureDrawn } from '../bindings/ChatPictureDrawn';
import type { ChatProposal } from '../bindings/ChatProposal';
import type { CodedError } from '../bindings/CodedError';
import type { ExternalReply } from '../bindings/ExternalReply';

/**
 * Where a proposal stands: waiting, being applied, applied, discarded, dropped (the project changed), replaced (a newer
 * one came: a project has one open proposal at a time), superseded (another app's that a newer one, shown before it,
 * had already replaced in Klipolo by the time it got here), or unshown (Rust applied it but the page couldn't take the
 * change in: never offered again).
 */
export type ProposalState =
  | 'open'
  | 'applying'
  | 'applied'
  | 'discarded'
  | 'dropped'
  | 'replaced'
  | 'superseded'
  | 'unshown';

/** A proposal in a reply, and what came of it. */
export interface ProposalPart {
  proposal: ChatProposal;
  state: ProposalState;
  /** Applied: its undo label, and the revision Apply made (its Undo chip works while that is the latest change). */
  undo?: CodedError;
  revision?: number;
  /** Open again after Apply was refused (not because the project moved): why, in one sentence, with Retry. */
  reason?: string;
  /**
   * When it took the place of a proposal that was open (`performance.now()`): an app proposed again. Apply waits
   * `SWAP_SETTLE_MS` from then, so a press meant for the card before never applies this one unread.
   */
  swappedAt?: number;
}

/** How long Apply is ignored after a proposal took the place of an open one. */
export const SWAP_SETTLE_MS = 500;

/**
 * Where a picture a reply asked for stands: being drawn, drawn (its card shows it with Use and Try again), being
 * placed, placed (its Undo chip), failed to draw (why, with Try again), or gone (it can't be placed any more: why, and
 * nothing to press).
 */
export type PictureState = 'drawing' | 'drawn' | 'placing' | 'placed' | 'failed' | 'gone';

/** A picture in a reply, and what came of it. */
export interface PicturePart {
  picture: ChatPicture;
  state: PictureState;
  /** The latest picture drawn (Try again draws another). */
  drawn?: ChatPictureDrawn;
  /** Why it failed or is gone, or why Use was refused, in one sentence. */
  reason?: string;
  /** Placed: its undo label, and the revision Use made (its Undo chip works while that is the latest change). */
  applied?: { undo: CodedError; revision: number };
  /** Try again is drawing a new picture; the one drawn before stays, with Use. */
  again?: boolean;
  /** Placed, but the page couldn't take the change in: no Undo chip, and it says so. */
  unshown?: boolean;
}

/** A sentence about chat that isn't a message, to say aloud: `seq` tells one saying from the next of the same words. */
export interface Notice {
  text: string;
  seq: number;
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
      /** The revision the reply landed on, as Rust says (its additions in): the one its moments' times are on. */
      revision: number;
      /** The project changed since it landed: its moments still seek, but can't be kept only (their times moved). */
      momentsStale: boolean;
      /** Additions applied at once: their undo label and the revision they made (the Undo chip). */
      applied: { undo: CodedError; revision: number } | null;
      proposal: ProposalPart | null;
      /** The pictures it asked for (stickers, title card backgrounds). */
      pictures: PicturePart[];
      /**
       * Another app's reply (MCP): the app's name, as it gave it. Its `text` is empty: the panel says what the app did.
       * Undefined for Klipolo's own.
       */
      client?: string;
      /** An app took back a step of its own: that step's undo label. */
      undid?: CodedError | null;
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
  /**
   * A sentence about chat that isn't a message (an Undo chip refused, a picture ready or placed), said aloud in its own
   * place beside the messages', so neither a thinking message nor its reply keeps it from being heard.
   */
  notice: Notice | null;
  /** Says `text` aloud (the Announcer), again even when it is the same words. */
  say: (text: string) => void;
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
  /** Draws picture `pictureId` of reply `messageId` (again, for Try again). */
  drawPicture: (messageId: string, pictureId: string) => Promise<void>;
  /** Use: places the picture drawn, as one undo step. */
  usePicture: (messageId: string, pictureId: string) => Promise<void>;
  /**
   * What another app did (`mcp-reply`), shown as a reply named for the app: its change merged, its proposal waiting
   * for Apply, its moments, its pictures drawn and placed. Taken in the order Rust told them.
   */
  receive: (external: ExternalReply) => Promise<void>;
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
    if (m.from === 'klipolo' && m.kind === 'reply' && m.proposal?.state === 'open')
      return { message: m, part: m.proposal };
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

/**
 * With an access code, the credit the top bar shows moved once a message or a picture was paid for: asked again. A
 * key's status has no credit to ask about.
 */
export function creditMoved() {
  if (useAi.getState().status?.mode === 'relay') void refreshAiStatus();
}

/** Each reset starts a new session: a reply or an apply that resolves after it belongs to the one before. */
let current = 0;
export const session = {
  now: () => current,
  next: () => {
    current += 1;
  },
};
