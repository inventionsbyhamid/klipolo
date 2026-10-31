import { create } from 'zustand';

interface ChatPanelStore {
  /** Chat is folded to its 56 px strip: never hidden, only collapsed. */
  collapsed: boolean;
  setCollapsed: (collapsed: boolean) => void;
  toggle: () => void;
}

/** How the chat panel shows: open, or folded to its strip. Command K opens it (`useUi.focusChat`). */
export const useChatPanel = create<ChatPanelStore>((set, get) => ({
  collapsed: false,
  setCollapsed: (collapsed) => {
    if (get().collapsed !== collapsed) set({ collapsed });
  },
  toggle: () => set((s) => ({ collapsed: !s.collapsed })),
}));
