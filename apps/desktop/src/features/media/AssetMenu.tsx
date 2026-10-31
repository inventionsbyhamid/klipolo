import { useLingui } from '@lingui/react/macro';
import type { Asset } from '../../bindings/Asset';
import { Menu, MenuContent, MenuItem, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import { addLayerAtPlayhead, addToMain, mediaFits } from './addMedia';

/**
 * A clip row's more menu, the keyboard's way to put a video in: Add to the end of the main track, or Add as a layer at
 * the playhead (over the clip there). Each runs the same action as a drag onto that lane.
 */
export function AssetMenu({ asset }: { asset: Asset }) {
  const { t } = useLingui();
  const { id, kind, name } = asset;
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          className="icon-btn row-action"
          aria-label={t({ id: 'media.row.more', message: `More for ${name}`, comment: '{name} is a file name' })}
        >
          <Icon name="more" />
        </button>
      </MenuTrigger>
      <MenuContent align="end">
        {mediaFits.main(kind) && (
          <MenuItem onSelect={() => addToMain(id)}>
            {t({ id: 'media.add.end', message: 'Add to the end', comment: 'Menu item: a video after the last clip' })}
          </MenuItem>
        )}
        {mediaFits.layer(kind) && (
          <MenuItem onSelect={() => addLayerAtPlayhead(id)}>
            {t({ id: 'media.add.layer', message: 'Add as a layer at the playhead', comment: 'Menu item' })}
          </MenuItem>
        )}
      </MenuContent>
    </Menu>
  );
}
