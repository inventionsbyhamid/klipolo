import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import { historyAction } from '../../lib/history';
import { FORMAT_SHORT } from '../../lib/layout';
import { labelText } from '../../lib/labels';
import { findClip, findItem, videoDurationUs } from '../../lib/timeline';
import { useChat } from '../../state/chat';
import { openProposal } from '../../state/chatMessages';
import { useProject } from '../../state/project';
import { useExport } from '../../state/export';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';
import { useUi } from '../../state/ui';
import { switchFormat } from '../editor/formats';
import { importFiles } from '../media/useImportMedia';
import { deleteBlock, deleteRange, useTimelineCommands } from '../timeline/commands';
import { searchWords, type Command } from './command';
import { useTransportCommands } from './useTransportCommands';

/** The formats 1, 2 and 3 switch to, in the preview's order. */
const FORMAT_KEYS: readonly FormatKind[] = ['landscape', 'portrait', 'square'];

/**
 * Every editor action with its current label and availability. The keyboard shortcuts, the player bar and the
 * timeline's block menus run them, so all agree; each id is stable. Values that change every frame (the playhead) are
 * read when a command runs. The timeline's are in `useTimelineCommands`, the transport's in `useTransportCommands`.
 *
 * Each hook builds its entries once per language and per what they read: a play, a selection or a view change rebuilds
 * only the entries that show it.
 */
export function useCommands(): Command[] {
  const { t } = useLingui();
  const snapshot = useProject((s) => s.snapshot);
  const selectedId = useSelection((s) => s.selectedId);
  const range = useRange((s) => s.range);
  /** The message whose proposal chat has open, if any (Command Enter applies it). */
  const proposalOpen = useChat((s) => openProposal(s.messages)?.message.id ?? null);
  const timeline = useTimelineCommands();
  const transport = useTransportCommands();

  const own = useMemo(() => {
    if (!snapshot) return null;
    const { project, history } = snapshot;
    const { undo, redo } = useProject.getState();
    const selectedItem = selectedId ? findItem(project, selectedId) : null;
    const selectedClip = selectedId && !selectedItem ? findClip(project, selectedId) : null;
    // A layer on a locked track can't be deleted: the action is unavailable rather than failing with a toast.
    const deletable = range !== null || Boolean((selectedItem && !selectedItem.track.locked) || selectedClip);
    // A project of layers alone is a video too.
    const hasVideo = videoDurationUs(project.timeline) > 0;
    const deleteLabel = range ? 'undo.delete.range' : selectedClip ? 'undo.delete.clip' : 'undo.delete.layer';

    const head: Command[] = [
      {
        id: 'focus-chat',
        action: 'askChat',
        group: 'edit',
        label: t({ id: 'command.focusChat', message: 'Ask chat', comment: 'Command: put the cursor in the chat box' }),
        shortcut: '⌘K',
        enabled: true,
        run: () => useUi.getState().focusChat(),
      },
      {
        id: 'apply-proposal',
        action: 'applyProposal',
        group: 'edit',
        label: t({
          id: 'command.applyProposal',
          message: 'Apply the proposal',
          comment: 'Command: apply the change chat proposed and is waiting on',
        }),
        shortcut: '⌘↩',
        enabled: proposalOpen !== null,
        run: () => {
          if (proposalOpen) void useChat.getState().apply(proposalOpen);
        },
      },
      {
        id: 'undo',
        action: 'history',
        group: 'edit',
        label: historyAction('undo', history.undoLabel, history.undoParams),
        shortcut: '⌘Z',
        enabled: history.canUndo,
        run: () => void undo(),
      },
      {
        id: 'redo',
        action: 'history',
        group: 'edit',
        label: historyAction('redo', history.redoLabel, history.redoParams),
        shortcut: '⇧⌘Z',
        enabled: history.canRedo,
        run: () => void redo(),
      },
      {
        id: 'import',
        action: 'importMedia',
        group: 'edit',
        label: t({ id: 'command.import', message: 'Add files' }),
        shortcut: '⌘O',
        enabled: true,
        // From a Finder drop, the files and where they go; else the file picker.
        run: (options) => void importFiles(options?.paths ?? null, options?.target ?? null),
      },
    ];
    const remove: Command = {
      id: 'delete',
      action: range ? 'deleteRanges' : selectedClip ? 'removeClip' : 'removeItem',
      group: 'edit',
      label: labelText(deleteLabel),
      shortcut: '⌫',
      keywords: searchWords(t({ id: 'command.delete.keywords', message: 'remove' })),
      enabled: deletable,
      // The selected time range first (its footage goes, in one step), else the selected clip or layer.
      run: () => {
        const now = useRange.getState().range;
        if (now) deleteRange(now);
        else if (selectedId) deleteBlock(selectedId, deleteLabel);
      },
    };
    const view: Command[] = FORMAT_KEYS.map((format, i) => ({
      id: `format-${i + 1}`,
      action: 'previewFormat',
      group: 'view',
      label: t({
        id: 'command.format',
        message: `Switch to ${FORMAT_SHORT[format]}`,
        comment: 'Command: make another format the main one',
      }),
      shortcut: String(i + 1),
      enabled: true,
      run: () => void switchFormat(format),
    }));
    const projectCommands: Command[] = [
      {
        id: 'export',
        action: 'exportVideo',
        group: 'project',
        label: t({ id: 'command.export', message: 'Export' }),
        shortcut: '⌘E',
        enabled: hasVideo,
        run: () => useExport.getState().setOpen(true),
      },
    ];
    return { head, remove, view, projectCommands };
  }, [t, snapshot, selectedId, range, proposalOpen]);

  return useMemo(
    () => (own ? [...own.head, own.remove, ...transport, ...own.view, ...own.projectCommands, ...timeline] : []),
    [own, transport, timeline],
  );
}
