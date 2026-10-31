import { useLingui } from '@lingui/react/macro';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { Project } from '../../bindings/Project';
import type { Track } from '../../bindings/Track';
import { IconButton } from '../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../design/Menu';
import { Icon, type IconName } from '../../layout/Icon';
import { contribLabel } from '../../lib/contrib';
import { findItem } from '../../lib/timeline';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { deleteBlock, duplicate, stepTrack } from '../timeline/commands';
import { AudioInspector } from './AudioInspector';
import { ImageInspector } from './ImageInspector';
import { commitLatest } from '../../state/latest';
import { TemplateInspector } from './TemplateInspector';
import { TextInspector } from './TextInspector';
import { TransitionInspector, useInspectedTransition } from './TransitionInspector';

/** What the inspector shows: a text, a picture, a title template, music or a sound effect. */
export type Inspected = 'text' | 'image' | 'template' | 'music' | 'sound';

/** The kind of `item` the inspector has fields for, or null (a clip, a video layer, a zoom). */
export function inspectedKind(item: Item): Inspected | null {
  const c = item.content;
  if (c.type === 'text') return 'text';
  if (c.type === 'image') return 'image';
  if (c.type === 'generator' && c.source.includes('/scene.')) return 'template';
  if (c.type === 'audio') return typeof c.duck === 'number' ? 'music' : 'sound';
  return null;
}

/** The selected layer the inspector shows, with its track; null when nothing it has fields for is selected. */
export function useInspected(): { item: Item; track: Track; kind: Inspected } | null {
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  return inspectedOf(project, selectedId);
}

export function inspectedOf(project: Project | undefined, selectedId: string | null) {
  if (!project || !selectedId) return null;
  const found = findItem(project, selectedId);
  const kind = found && inspectedKind(found.item);
  return found && kind ? { ...found, kind } : null;
}

const ICONS: Record<Inspected, IconName> = {
  text: 'text',
  image: 'image',
  template: 'text',
  music: 'music',
  sound: 'pulse',
};

/**
 * The inspector: a raised card docked to the right inside the preview while an overlay or a sound is
 * selected. Its header has the kind's icon and name, a more menu (Duplicate, Bring forward, Delete) and a close button;
 * its body the fields of that kind, scrolling with a fade at the bottom. On a locked track it says so and offers
 * Unlock. Changes apply at once and each is one undo step.
 */
export function Inspector({ format }: { format: FormatKind }) {
  const { t } = useLingui();
  const inspected = useInspected();
  const transition = useInspectedTransition();
  const primary = useProject((s) => s.snapshot?.project.settings.primaryFormat ?? 'landscape');
  // A transition selected at a cut: its own card (`TransitionInspector`).
  if (transition) return <TransitionInspector key={transition.clip.id} {...transition} />;
  if (!inspected) return null;
  const { item, track, kind } = inspected;
  const content = item.content;
  const title =
    kind === 'text'
      ? t({ id: 'inspector.text', message: 'Title' })
      : kind === 'image'
        ? t({ id: 'inspector.image', message: 'Image' })
        : kind === 'music'
          ? t({ id: 'inspector.music', message: 'Music' })
          : kind === 'sound'
            ? t({ id: 'inspector.sound', message: 'Sound' })
            : content.type === 'generator'
              ? contribLabel(content.source, 'scene')
              : '';
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
        <h3>
          <Icon name={ICONS[kind]} />
          {title}
        </h3>
        <Menu>
          <MenuTrigger asChild>
            <button type="button" className="icon-btn" aria-label={t({ id: 'inspector.more', message: 'More' })}>
              <Icon name="more" />
            </button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem onSelect={() => duplicate(item.id)} disabled={track.locked}>
              {t({ id: 'inspector.duplicate', message: 'Duplicate' })}
            </MenuItem>
            {(kind === 'text' || kind === 'image' || kind === 'template') && (
              <MenuItem onSelect={() => stepTrack(item.id, 1)} disabled={track.locked}>
                {t({ id: 'inspector.forward', message: 'Bring forward' })}
              </MenuItem>
            )}
            <MenuSeparator />
            <MenuItem onSelect={() => deleteBlock(item.id, 'undo.delete.layer')} disabled={track.locked}>
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
        {track.locked ? (
          <Locked track={track} />
        ) : (
          <>
            {content.type === 'text' && (
              <TextInspector
                key={item.id}
                item={item}
                content={content}
                format={format}
                primary={primary}
                locked={track.locked}
              />
            )}
            {content.type === 'image' && <ImageInspector key={item.id} item={item} format={format} primary={primary} />}
            {content.type === 'generator' && (
              <TemplateInspector key={item.id} item={item} content={content} locked={track.locked} />
            )}
            {content.type === 'audio' && <AudioInspector key={item.id} item={item} content={content} />}
          </>
        )}
      </div>
    </aside>
  );
}

/** A locked track's layer: said, with Unlock. */
function Locked({ track }: { track: Track }) {
  const { t } = useLingui();
  return (
    <div className="insp-locked">
      <p className="field-note">{t({ id: 'toolbar.locked', message: 'Its track is locked.' })}</p>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        onClick={() =>
          void commitLatest('timeline.track.unlock', () => [
            { type: 'updateTrack', id: track.id, patch: { locked: false } },
          ])
        }
      >
        {t({ id: 'toolbar.unlock', message: 'Unlock' })}
      </button>
    </div>
  );
}
