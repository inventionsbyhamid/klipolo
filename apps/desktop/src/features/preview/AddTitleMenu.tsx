import { useLingui } from '@lingui/react/macro';
import { useRef } from 'react';
import { Menu, MenuContent, MenuItem, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import { addTitle, templateName } from '../media/place';
import { useAddText } from './useAddText';

/**
 * Add title, in the preview's header: Text opens a new box on the canvas to type in (it goes in once it has words);
 * Lower third and Title card add Klipolo's title templates at the playhead, filling the frame in every format.
 */
export function AddTitleMenu({ disabled }: { disabled?: boolean }) {
  const { t } = useLingui();
  const addText = useAddText();
  const label = t({ id: 'title.add', message: 'Add title' });
  /** The item picked: run once the menu has closed, so the new box's cursor isn't taken back by the menu's focus. */
  const picked = useRef<(() => void) | null>(null);
  const pick = (run: () => void) => () => {
    picked.current = run;
  };
  return (
    <Menu>
      <MenuTrigger asChild disabled={disabled}>
        <button type="button" className="icon-btn" aria-label={label} title={label}>
          <Icon name="text" />
        </button>
      </MenuTrigger>
      <MenuContent
        align="end"
        onCloseAutoFocus={(e) => {
          const run = picked.current;
          picked.current = null;
          if (!run) return;
          // Focus goes where the item puts it (the new box), never back to the button.
          e.preventDefault();
          run();
        }}
      >
        <MenuItem onSelect={pick(() => void addText())}>{t({ id: 'title.add.text', message: 'Text' })}</MenuItem>
        <MenuItem onSelect={pick(() => void addTitle({ type: 'template', template: 'lowerThird' }))}>
          {templateName.lowerThird()}
        </MenuItem>
        <MenuItem onSelect={pick(() => void addTitle({ type: 'template', template: 'titleCard' }))}>
          {templateName.titleCard()}
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}
