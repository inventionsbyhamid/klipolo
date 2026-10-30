import { useState, type MouseEvent, type ReactElement } from 'react';
import { ContextMenuContent, ContextMenuRoot, ContextMenuTrigger } from '../../../design/ContextMenu';
import { findItem } from '../../../lib/timeline';
import { useProject } from '../../../state/project';
import { useSelection } from '../../../state/selection';
import { useTimelineFocus } from '../../../state/timelineFocus';
import { dragging } from '../drag/controller';
import { blockElement } from '../drag/registry';
import { cutClipId } from '../keys/laneOrder';
import { HEADER_PX } from '../layout';
import { BlockMenuItems, blockMenuLabel } from './BlockMenuItems';
import { setKeyboardMenu } from './menuKeyboard';

export { keyboardMenuOpen } from './menuKeyboard';

/** The block a menu is open on, and whether the keyboard opened it (focus goes back to the block when it closes). */
interface Target {
  id: string;
  kind: 'clip' | 'layer';
  keyboard: boolean;
}

/** Set while `openBlockMenu` dispatches its event, so the menu knows the keyboard opened it. */
let fromKeyboard = false;

/**
 * Shift+F10 or the Menu key on block `el`: its menu opens at the middle of the part of it in view (as a right-click
 * there would).
 */
export function openBlockMenu(el: HTMLElement, scroller: HTMLElement | null) {
  const r = el.getBoundingClientRect();
  const view = scroller?.getBoundingClientRect();
  let x = r.left + r.width / 2;
  if (view && view.width > 0) x = Math.min(Math.max(x, view.left + HEADER_PX + 8), view.right - 8);
  fromKeyboard = true;
  try {
    el.dispatchEvent(
      new window.MouseEvent('contextmenu', {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: r.top + r.height / 2,
      }),
    );
  } finally {
    fromKeyboard = false;
  }
}

/**
 * Every block's menu (right-click, Shift+F10, the Menu key): one menu for the whole timeline, on the element holding
 * its lanes (`children`), opened for the block it was asked on. Its items mount only while it is open, so the hundreds
 * of blocks on screen carry no menu of their own (a Radix menu per block costs each a document key listener and
 * several hooks: mounting 280 blocks took about 5× as long, and every key press ran 280 listeners). Elsewhere in the
 * lanes, no menu opens (nor the system's).
 */
export function BlockMenu({ children }: { children: ReactElement }) {
  const [target, setTarget] = useState<Target | null>(null);
  const onContextMenu = (e: MouseEvent) => {
    const el = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-block-id]') : null;
    const id = el?.dataset.blockId;
    const project = useProject.getState().snapshot?.project;
    // A cut is a place between clips, not a block with actions of its own: its button, chip or Enter act on it.
    if (!id || !project || dragging() || cutClipId(id) !== null) return e.preventDefault();
    // The menu acts on the block it was opened on: it is selected (the playhead stays, so Split splits there).
    useSelection.getState().select(id);
    setKeyboardMenu(fromKeyboard);
    setTarget({ id, kind: findItem(project, id) ? 'layer' : 'clip', keyboard: fromKeyboard });
  };
  return (
    <ContextMenuRoot>
      <ContextMenuTrigger asChild onContextMenu={onContextMenu}>
        {children}
      </ContextMenuTrigger>
      <ContextMenuContent
        label={target ? blockMenuLabel(target.id) : undefined}
        onCloseAutoFocus={(e) => {
          // After keyboard use, back to the block, or to where focus went if its item took the block away (Delete,
          // Cut: the timeline moved focus and the selection on, as ⌫ does); after a click, focus isn't moved (Space
          // then plays, as `Menu`).
          e.preventDefault();
          setKeyboardMenu(false);
          // An item opened Sync's picker, which has focus now (taking it back would close it): it hands the keyboard
          // back to the block itself when it closes.
          if (!target?.keyboard) return;
          const back = blockElement(target.id) ?? blockElement(useTimelineFocus.getState().activeId ?? '');
          back?.focus({ preventScroll: true });
        }}
      >
        {target && <BlockMenuItems blockId={target.id} kind={target.kind} />}
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}
