import type { ChatApplied } from '../bindings/ChatApplied';
import type { ChatOutcome } from '../bindings/ChatOutcome';
import type { ChatPictureDrawn } from '../bindings/ChatPictureDrawn';
import type { ChatRequest } from '../bindings/ChatRequest';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { call } from './call';

/**
 * What the chat panel asks of Rust: the planner's commands (`crates/klipolo-app`'s `chat` shapes, filled in with the
 * planner). Everything the model wrote reaches the page already cleaned of em and en dashes (`klipolo_app::chat`), so
 * the page shows it as it is. Edits come back as patches the page merges (`useProject.adopt`); the Undo chip under them
 * takes one back through `undoLast`, which Rust allows only while it is still chat's last change.
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
  /**
   * Takes back chat's change that made `revision` (`ChatApplied.change.history.revision`): what the undo changed.
   * Rejects with `project.moved` once that change is no longer chat's last.
   */
  undoLast(revision: number): Promise<SnapshotPatch>;
  /**
   * Draws the picture a reply asked for (`ChatReply.pictures`), or draws it again (Try again): about 35 s, into the
   * project's media folder, not into the project. Rejects with a plain sentence when it can't be drawn.
   */
  drawPicture(pictureId: string): Promise<ChatPictureDrawn>;
  /**
   * Use: places the drawing at `path` (the one the card shows: a newer one may have landed) in the project as one undo
   * step. Rejects with a plain sentence when it can't be placed any more (the moment it was for was cut, its title is
   * gone, that drawing isn't there), or with `project.moved`.
   */
  usePicture(pictureId: string, path: string): Promise<ChatApplied>;
}

/** The planner's commands in the app. */
export const tauriChat: ChatBridge = {
  send: (request) => call<ChatOutcome>('chat_send', { request }),
  apply: (proposalId) => call<ChatApplied>('chat_apply_proposal', { id: proposalId }),
  discard: (proposalId) => call<void>('chat_discard_proposal', { id: proposalId }),
  reset: () => call<void>('chat_reset'),
  undoLast: (revision) => call<SnapshotPatch>('chat_undo_last', { revision }),
  drawPicture: (pictureId) => call<ChatPictureDrawn>('chat_draw_picture', { id: pictureId }),
  usePicture: (pictureId, path) => call<ChatApplied>('chat_use_picture', { id: pictureId, path }),
};

let current: ChatBridge = tauriChat;

/** The chat bridge in use: the app's, unless a fake was installed (tests; a development build's fake chat). */
export const chatBridge = (): ChatBridge => current;

/** Uses `bridge` for chat from now on (`tauriChat` puts the app's back). */
export function installChatBridge(bridge: ChatBridge) {
  current = bridge;
}
