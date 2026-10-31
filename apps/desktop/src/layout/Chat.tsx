import { ChatPanel } from '../features/chat/ChatPanel';

/**
 * The chat panel in the window's right column: messages, the composer, the selection and undo chips, and
 * the proposal card. Command K opens it and puts the cursor in the composer (`useUi.focusChat`).
 */
export function Chat() {
  return <ChatPanel />;
}
