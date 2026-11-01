import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import { useChatPanel } from '../../state/chatPanel';
import { useLibraryPanel } from '../../state/libraryPanel';
import { useUi } from '../../state/ui';
import { searchWords, type Command } from './command';

/**
 * The panels' commands: Control Command I folds chat to its strip or opens it with the cursor in its composer, as
 * Command K does; Control Command S folds the library to its rail or opens it. Each label says what a press does.
 */
export function usePanelCommands(): Command[] {
  const { t } = useLingui();
  const chatFolded = useChatPanel((s) => s.collapsed);
  const libraryFolded = useLibraryPanel((s) => s.collapsed);
  return useMemo(
    () => [
      {
        id: 'toggle-chat',
        action: 'viewOptions',
        group: 'view',
        label: chatFolded
          ? t({ id: 'chatPanel.open', message: 'Open chat' })
          : t({ id: 'chatPanel.collapse', message: 'Collapse chat' }),
        shortcut: '⌃⌘I',
        keywords: searchWords(
          t({
            id: 'command.chat.keywords',
            message: 'sidebar, assistant, panel',
            comment: 'Comma-separated search words for opening or collapsing the chat',
          }),
        ),
        enabled: true,
        run: () => {
          if (useChatPanel.getState().collapsed) useUi.getState().focusChat();
          else useChatPanel.getState().setCollapsed(true);
        },
      },
      {
        id: 'toggle-side-panel',
        action: 'viewOptions',
        group: 'view',
        label: libraryFolded
          ? t({ id: 'library.open', message: 'Open library' })
          : t({ id: 'library.collapse', message: 'Collapse library' }),
        shortcut: '⌃⌘S',
        keywords: searchWords(
          t({
            id: 'command.library.keywords',
            message: 'sidebar, media, files, panel',
            comment: 'Comma-separated search words for opening or collapsing the library',
          }),
        ),
        enabled: true,
        run: () => useLibraryPanel.getState().toggle(),
      },
    ],
    [t, chatFolded, libraryFolded],
  );
}
