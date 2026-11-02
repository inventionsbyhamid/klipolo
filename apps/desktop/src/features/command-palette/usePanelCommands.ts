import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import { useChatPanel } from '../../state/chatPanel';
import { cardHasColumn, useLibraryPanel } from '../../state/libraryPanel';
import { useSelection } from '../../state/selection';
import { useUi } from '../../state/ui';
import { searchWords, type Command } from './command';

/**
 * The panels' commands: Control Command I folds chat to its strip or opens it with the cursor in its composer, as
 * Command K does; Command B (or Control Command S) folds the library to its rail or opens it. While the inspector card
 * has the column, it folds the column and the card steps aside. Each label says what a press does.
 */
export function usePanelCommands(): Command[] {
  const { t } = useLingui();
  const chatFolded = useChatPanel((s) => s.collapsed);
  const aside = useLibraryPanel((s) => s.aside);
  const carded = useSelection((s) => cardHasColumn(aside, s));
  const libraryFolded = useLibraryPanel((s) => s.collapsed) && !carded;
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
        // Control Command S works too (`CONTROL_COMMAND`).
        shortcut: '⌘B',
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
