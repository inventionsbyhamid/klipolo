import { useLingui } from '@lingui/react/macro';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import { IconButton } from '../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import { sourceLabel } from '../../lib/timeline';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { deleteBlock, duplicate } from '../timeline/commands';
import { FramingField } from './FramingField';

/**
 * The clip card: the inspector for a selected main-track clip, in the left column like the others. Its header has the
 * film icon and the clip's file name, a more menu (Duplicate, Delete: the timeline's own commands; a main clip has no
 * layer order) and a close button. Its fields: Fit or Fill in the format shown (`FramingField`). Each change is one
 * undo step.
 */
export function ClipInspector({ clip, format }: { clip: Clip; format: FormatKind }) {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  if (!project) return null;
  const title = sourceLabel(project, clip.source);
  const close = () => useSelection.getState().select(null);
  return (
    <aside
      className="inspector"
      aria-label={t({ id: 'inspector.label', message: `${title} settings`, comment: '{title}: Title, Image, Music' })}
      onKeyDown={(e) => {
        // Escape in a field puts its value back (the field handles it); elsewhere it closes the inspector.
        if (e.key === 'Escape' && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) {
          e.stopPropagation();
          close();
        }
      }}
    >
      <div className="insp-head">
        <h3 title={title}>
          <Icon name="film" />
          <span className="insp-name">{title}</span>
        </h3>
        <Menu>
          <MenuTrigger asChild>
            <button type="button" className="icon-btn" aria-label={t({ id: 'inspector.more', message: 'More' })}>
              <Icon name="more" />
            </button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem onSelect={() => duplicate(clip.id)}>
              {t({ id: 'inspector.duplicate', message: 'Duplicate' })}
            </MenuItem>
            <MenuSeparator />
            <MenuItem onSelect={() => deleteBlock(clip.id, 'undo.delete.clip')}>
              {t({ id: 'inspector.delete', message: 'Delete' })}
            </MenuItem>
          </MenuContent>
        </Menu>
        <IconButton
          icon={<Icon name="x" />}
          label={t({ id: 'inspector.close', message: 'Close' })}
          shortcut="Esc"
          onClick={close}
        />
      </div>
      <div className="insp-body">
        <FramingField clip={clip} format={format} />
      </div>
    </aside>
  );
}
