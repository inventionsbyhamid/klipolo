import { useLingui } from '@lingui/react/macro';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { Project } from '../../bindings/Project';
import type { Track } from '../../bindings/Track';
import { IconButton } from '../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../design/Menu';
import { Icon, type IconName } from '../../layout/Icon';
import { contribLabel } from '../../lib/contrib';
import { findClip, findItem } from '../../lib/timeline';
import { useProject } from '../../state/project';
import { cameraClipId, groupOf, useSelection } from '../../state/selection';
import { deleteBlock, duplicate, stepTrack, trackStepOps } from '../timeline/commands';
import { AdjustmentInspector } from './AdjustmentInspector';
import { AudioInspector } from './AudioInspector';
import { CameraInspector } from './CameraInspector';
import { ClipInspector } from './ClipInspector';
import { BackToLibrary, cardEscape } from './common';
import { GeneratorInspector } from './GeneratorInspector';
import { GroupInspector } from './GroupInspector';
import { ImageInspector } from './ImageInspector';
import { commitLatest } from '../../state/latest';
import { TemplateInspector } from './TemplateInspector';
import { TextInspector } from './TextInspector';
import { TransitionInspector, useInspectedTransition } from './TransitionInspector';
import { VideoInspector } from './VideoInspector';
import { ZoomInspector } from './ZoomInspector';

/**
 * What the inspector shows: a text, a picture, a title template, music, a sound effect, a video layer, or a zoom,
 * adjustment or generator layer (made by a recording or by chat).
 */
export type Inspected =
  | 'text'
  | 'image'
  | 'template'
  | 'music'
  | 'sound'
  | 'video'
  | 'zoom'
  | 'adjustment'
  | 'generator';

/** The kind of `item` the inspector has fields for: every layer has a card; null for a kind it doesn't know. */
export function inspectedKind(item: Item): Inspected | null {
  const c = item.content;
  if (c.type === 'text') return 'text';
  if (c.type === 'image') return 'image';
  if (c.type === 'video') return 'video';
  if (c.type === 'generator') return c.source.includes('/scene.') ? 'template' : 'generator';
  if (c.type === 'audio') return typeof c.duck === 'number' ? 'music' : 'sound';
  if (c.type === 'zoom') return 'zoom';
  if (c.type === 'adjustment') return 'adjustment';
  // A kind the bindings gain fails `tsc` here; at run time (a project from a newer version) it gets no card.
  const unknown: never = c;
  void unknown;
  return null;
}

/** A selected main-track clip: the clip card shows it (it has no layer or track). */
export interface InspectedClip {
  kind: 'clip';
  clip: Clip;
}

/** A recording clip's camera, selected on the video: the Camera card shows it. */
export interface InspectedCamera {
  kind: 'camera';
  clip: Clip;
}

/**
 * The selected layer the inspector shows, with its track, the selected main-track clip, or a recording clip's camera;
 * null when nothing it has fields for is selected.
 */
export function useInspected():
  | { item: Item; track: Track; kind: Inspected }
  | InspectedClip
  | InspectedCamera
  | null {
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  return inspectedOf(project, selectedId);
}

export function inspectedOf(project: Project | undefined, selectedId: string | null) {
  if (!project || !selectedId) return null;
  const cameraOf = cameraClipId(selectedId);
  if (cameraOf !== null) {
    const clip = findClip(project, cameraOf);
    return clip?.camera ? { kind: 'camera' as const, clip } : null;
  }
  const found = findItem(project, selectedId);
  const clip = found ? null : findClip(project, selectedId);
  if (clip) return { kind: 'clip' as const, clip };
  const kind = found && inspectedKind(found.item);
  return found && kind ? { ...found, kind } : null;
}

const ICONS: Record<Inspected, IconName> = {
  text: 'text',
  image: 'image',
  template: 'text',
  music: 'music',
  sound: 'pulse',
  video: 'film',
  zoom: 'zoom-in',
  adjustment: 'layers',
  generator: 'image',
};

/**
 * The inspector: a raised card that fills the left column over the library while a layer is selected
 * (`Library`). Its header has Back to library, the kind's icon and name, a more menu (Duplicate, Bring forward and
 * Send back for a layer drawn over the video, Delete) and a close button; its body the fields of that kind, scrolling
 * with a fade at the bottom. On a locked track it says so and offers Unlock. Changes apply at once and each is one undo step.
 * A main-track clip has a card of its own (`ClipInspector`), and so has a recording's camera (`CameraInspector`).
 */
export function Inspector({ format }: { format: FormatKind }) {
  const { t } = useLingui();
  const inspected = useInspected();
  const transition = useInspectedTransition();
  const primary = useProject((s) => s.snapshot?.project.settings.primaryFormat ?? 'landscape');
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  const selectedIds = useSelection((s) => s.selectedIds);
  // Several clips and layers selected: the card for all of them (`GroupInspector`).
  const group = groupOf({ selectedId, selectedIds });
  if (group) return <GroupInspector key={group.join(' ')} ids={group} />;
  // A transition selected at a cut: its own card (`TransitionInspector`).
  if (transition) return <TransitionInspector key={transition.clip.id} {...transition} />;
  if (!inspected) return null;
  if (inspected.kind === 'clip') return <ClipInspector key={inspected.clip.id} clip={inspected.clip} format={format} />;
  if (inspected.kind === 'camera') {
    const { clip } = inspected;
    return (
      clip.camera && (
        <CameraInspector key={`camera:${clip.id}`} clip={clip} camera={clip.camera} format={format} />
      )
    );
  }
  const { item, track, kind } = inspected;
  // A layer goes a track up or down only when one of its kind is there to take it (not on a locked track).
  const canStep = (direction: 1 | -1) => !!project && trackStepOps(project, item.id, direction).length > 0;
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
            : kind === 'video'
              ? t({ id: 'inspector.video', message: 'Video layer' })
              : kind === 'zoom'
                ? t({ id: 'inspector.zoom', message: 'Zoom', comment: 'A zoom layer' })
                : kind === 'adjustment'
                  ? t({ id: 'inspector.adjustment', message: 'Adjustment layer' })
                  : content.type === 'generator'
                    ? contribLabel(content.source, kind === 'template' ? 'scene' : 'generator')
                    : '';
  // A layer drawn over the video goes over or under the others; a zoom has a track of its own.
  const ordered = kind !== 'zoom' && track.kind === 'overlay';
  const close = () => useSelection.getState().select(null);
  return (
    <aside
      className="inspector"
      aria-label={t({ id: 'inspector.label', message: `${title} settings`, comment: '{title}: Title, Image, Music' })}
      onKeyDown={cardEscape(close)}
    >
      <div className="insp-head">
        <BackToLibrary />
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
            {ordered && (
              <>
                <MenuItem onSelect={() => stepTrack(item.id, 1)} disabled={!canStep(1)}>
                  {t({ id: 'inspector.forward', message: 'Bring forward' })}
                </MenuItem>
                <MenuItem onSelect={() => stepTrack(item.id, -1)} disabled={!canStep(-1)}>
                  {t({ id: 'inspector.back', message: 'Send back' })}
                </MenuItem>
              </>
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
            {content.type === 'generator' &&
              (kind === 'template' ? (
                <TemplateInspector key={item.id} item={item} content={content} locked={track.locked} />
              ) : (
                <GeneratorInspector key={item.id} item={item} content={content} />
              ))}
            {content.type === 'audio' && <AudioInspector key={item.id} item={item} content={content} />}
            {content.type === 'video' && (
              <VideoInspector key={item.id} item={item} content={content} format={format} primary={primary} />
            )}
            {content.type === 'zoom' && <ZoomInspector key={item.id} item={item} content={content} />}
            {content.type === 'adjustment' && <AdjustmentInspector key={item.id} item={item} />}
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
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
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
