import type { ChatApplied } from '../bindings/ChatApplied';
import type { ChatOutcome } from '../bindings/ChatOutcome';
import type { ChatRequest } from '../bindings/ChatRequest';
import { call } from './call';

/**
 * What the chat panel asks of Rust: the planner's commands (`crates/klipolo-app`'s `chat` shapes, filled in with the
 * planner). Everything the model wrote reaches the page already cleaned of em and en dashes (`klipolo_app::chat`), so
 * the page shows it as it is. Edits come back as patches the page merges (`useProject.adopt`); the Undo chip under them
 * takes them back through `useProject.undoIf`, so no command of its own.
 */
export interface ChatBridge {
  /**
   * One message (or "Keep only these", `keepOnly`): a reply (its words, the additions it applied at once as one undo
   * step, what the model's own undo changed, the removals it proposes, the moments it points at), or why nothing
   * happened, the project untouched. Never rejects for the model's errors: those are `{ type: 'failed' }`.
   */
  send(request: ChatRequest): Promise<ChatOutcome>;
  /**
   * Applies proposal `proposalId` as one undo step: what changed, and its undo label. Rejects with `project.moved` when
   * the project changed since the proposal was made (it is dropped), or a plain sentence.
   */
  apply(proposalId: string): Promise<ChatApplied>;
  /** Drops proposal `proposalId`: nothing changes. */
  discard(proposalId: string): Promise<void>;
  /** A new chat: Rust forgets the conversation it keeps for the model. */
  reset(): Promise<void>;
}

/** The planner's commands in the app. */
export const tauriChat: ChatBridge = {
  send: (request) => call<ChatOutcome>('chat_send', { request }),
  apply: (proposalId) => call<ChatApplied>('chat_apply_proposal', { id: proposalId }),
  discard: (proposalId) => call<void>('chat_discard_proposal', { id: proposalId }),
  reset: () => call<void>('chat_reset'),
};

let current: ChatBridge = tauriChat;

/** The chat bridge in use: the app's, unless a fake was installed (tests; a development build's fake chat). */
export const chatBridge = (): ChatBridge => current;

/** Uses `bridge` for chat from now on (`tauriChat` puts the app's back). */
export function installChatBridge(bridge: ChatBridge) {
  current = bridge;
}
