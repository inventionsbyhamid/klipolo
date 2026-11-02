import { useLingui } from '@lingui/react/macro';
import { Volume2, VolumeX } from 'lucide-react';
import { Button } from '../../design/Button';
import { IconButton } from '../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import clipSettings from '../../lib/clip-settings.json';
import { useClipboard } from '../../state/clipboard';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import {
  allEffects,
  allSound,
  copyGroup,
  countOf,
  cutGroup,
  deleteGroup,
  duplicateGroup,
  groupMuted,
  groupVolume,
  groupVolumeOps,
  itemsWord,
  membersOf,
  muteGroup,
  pasteClipboard,
  pasteEffectsGroup,
} from '../timeline/commands';
import { BackToLibrary, cardEscape } from './common';
import { Field } from './fields';
import { ToolSlider } from './ToolSlider';

const percent = (v: number) => `${Math.round(v * 100)}%`;
/** The model's volume range and step (`klipolo_model::clip_settings`). */
const VOLUME = clipSettings.volume;

/**
 * The card for several selected clips and layers ("3 items"): only the fields they all share. Volume and Mute when
 * every one of them makes sound (volumes that differ read Mixed until one is set for all), Effects when every one can
 * carry effects (Paste effects puts the copied stack on each), and in its more menu Copy, Cut, Paste, Duplicate and
 * Delete. Each change is one undo step for the whole group ("Mute 3 items").
 */
export function GroupInspector({ ids }: { ids: readonly string[] }) {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const clipboard = useClipboard();
  if (!project) return null;
  const m = membersOf(project, ids);
  const count = countOf(m);
  const title = itemsWord(count);
  const sound = allSound(project, m);
  const effects = allEffects(m);
  const muted = sound && groupMuted(project, m);
  const volume = sound ? (groupVolume(project, m) ?? null) : null;
  const mixed = t({
    id: 'inspector.group.mixed',
    message: 'Mixed',
    comment: 'The selected sounds have different volumes',
  });
  const pastable = clipboard.projectId === project.id && clipboard.entry !== null;
  const effectsPastable = clipboard.projectId === project.id && clipboard.effects !== null;
  const close = () => useSelection.getState().select(null);
  /** The group's volume ops, built from the project as it is when the change goes in. */
  const volumeOps = (v: number) => {
    const now = useProject.getState().snapshot?.project;
    return now ? groupVolumeOps(now, membersOf(now, ids), v) : [];
  };
  return (
    <aside
      className="inspector"
      aria-label={t({ id: 'inspector.group.label', message: `${title} settings`, comment: '{title}: 3 items' })}
      onKeyDown={cardEscape(close)}
    >
      <div className="insp-head">
        <BackToLibrary />
        <h3>
          <Icon name="layers" />
          {title}
        </h3>
        <Menu>
          <MenuTrigger asChild>
            <button type="button" className="icon-btn" aria-label={t({ id: 'inspector.more', message: 'More' })}>
              <Icon name="more" />
            </button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem onSelect={copyGroup}>{t({ id: 'timeline.copy', message: 'Copy', comment: 'Command' })}</MenuItem>
            <MenuItem onSelect={cutGroup}>
              {t({ id: 'inspector.group.cut', message: 'Cut', comment: 'Menu item' })}
            </MenuItem>
            <MenuItem onSelect={pasteClipboard} disabled={!pastable}>
              {t({ id: 'inspector.group.paste', message: 'Paste', comment: 'Menu item' })}
            </MenuItem>
            <MenuItem onSelect={duplicateGroup}>{t({ id: 'inspector.duplicate', message: 'Duplicate' })}</MenuItem>
            <MenuSeparator />
            <MenuItem onSelect={deleteGroup}>{t({ id: 'inspector.delete', message: 'Delete' })}</MenuItem>
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
        {sound && (
          <Field label={t({ id: 'volume.label', message: 'Volume' })}>
            <div className="volume-row">
              <ToolSlider
                key={ids.join(' ')}
                label={t({ id: 'volume.label', message: 'Volume' })}
                commitLabel={{ id: 'timeline.group.volume', params: { count } }}
                value={volume ?? 1}
                min={VOLUME.min}
                max={VOLUME.max}
                step={VOLUME.step}
                // Volumes that differ read Mixed until one is set for all.
                format={(v) => (volume === null && v === 1 ? mixed : percent(v))}
                preview={false}
                ops={volumeOps}
              />
              <IconButton
                className="icon-btn-sm"
                icon={muted ? VolumeX : Volume2}
                label={t({ id: 'volume.mute', message: 'Mute' })}
                tooltip={
                  muted ? t({ id: 'volume.unmute', message: 'Unmute' }) : t({ id: 'volume.mute', message: 'Mute' })
                }
                active={muted}
                onClick={() => muteGroup(!muted)}
              />
            </div>
          </Field>
        )}
        {effects && (
          <Field
            label={t({
              id: 'inspector.group.effects',
              message: 'Effects',
              comment: 'A field of the card for several items',
            })}
          >
            <Button
              size="sm"
              aria-disabled={!effectsPastable || undefined}
              title={
                effectsPastable
                  ? undefined
                  : t({ id: 'timeline.pasteEffects.none', message: 'Copy a clip or layer that has effects first.' })
              }
              onClick={() => effectsPastable && pasteEffectsGroup()}
            >
              {t({ id: 'inspector.group.pasteEffects', message: 'Paste effects' })}
            </Button>
          </Field>
        )}
        {!sound && !effects && (
          <p className="field-note">
            {t({
              id: 'inspector.group.nothingShared',
              message: 'These have no settings in common. Delete, copy or duplicate them from More.',
            })}
          </p>
        )}
      </div>
    </aside>
  );
}
