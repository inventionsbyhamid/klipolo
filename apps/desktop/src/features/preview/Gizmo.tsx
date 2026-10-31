import { useLingui } from '@lingui/react/macro';
import type { FormatKind } from '../../bindings/FormatKind';
import { cn } from '../../design/cn';
import type { Size } from '../../lib/geometry';
import { videoDurationUs } from '../../lib/timeline';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { focusInspectorText } from '../inspector/focus';
import { GizmoHandles } from './GizmoHandles';
import { useTextEdit } from './textEdit';
import type { GizmoTarget } from './gizmoTargets';
import { boxStyle } from './layerBox';
import { useGizmoDrag } from './useGizmoDrag';
import { useNudge } from './useNudge';

/** Below this size (CSS px) the handles sit outside the box and grab only what they draw, so the body can be moved. */
const COMPACT_PX = 32;
/** Holding these alone mustn't end a nudge burst (Shift and Option change what the next arrow does). */
const MODIFIERS = ['Shift', 'Alt', 'Meta', 'Control'];

interface GizmoProps {
  target: GizmoTarget;
  /** Everything pickable at the playhead (what a move snaps to). */
  others: GizmoTarget[];
  selected: boolean;
  canvas: Size;
  format: FormatKind;
  primary: FormatKind;
  playing: boolean;
  /** Above the layers drawn before it: the selected layer and every layer drawn after it keep their draw order. */
  lifted?: boolean;
}

/**
 * One pickable layer over the engine's pixels: pressing selects it (pausing playback) and dragging moves
 * it in the same gesture. Selected, it shows a paper outline and eight handles to resize it, and its keys: arrows
 * nudge, Enter types a title's words on the canvas (as a double-click does), Space plays, Escape lets go. A title
 * template's words are typed in the inspector instead (its place is the frame). Layers on locked tracks select but
 * don't move, and their titles aren't typed.
 */
export function Gizmo({
  target,
  others,
  selected,
  canvas,
  format,
  primary,
  playing,
  lifted = false,
}: GizmoProps) {
  const { t } = useLingui();
  const nudge = useNudge(target, format);
  const { ref, current, opsFor, dragging, handlers, cancel } = useGizmoDrag(
    target,
    others,
    canvas,
    format,
    primary,
    nudge.placement,
  );
  const fixed = target.locked || !target.movable;
  const compact = Math.min(current.box.w * canvas.width, current.box.h * canvas.height) < COMPACT_PX;
  const layer = target.name;
  // Only the name it shows is looked up.
  const name =
    target.label === 'text'
      ? t({ id: 'gizmo.kind.text', message: `Text: ${layer}`, comment: '{layer} is the title’s words' })
      : target.label === 'scene'
        ? t({ id: 'gizmo.kind.scene', message: `Title: ${layer}`, comment: '{layer} is the template’s name' })
        : target.label === 'image'
          ? t({
              id: 'gizmo.kind.image',
              message: `Image: ${layer}`,
              comment: '{layer} is the image’s file name',
            })
          : t({
              id: 'gizmo.kind.video',
              message: `Video: ${layer}`,
              comment: '{layer} is the video’s file name',
            });
  const label = selected
    ? target.locked
      ? t({
          id: 'gizmo.selected.locked',
          message: `${name}. Locked`,
          comment: 'A selected layer on a locked track',
        })
      : fixed
        ? name
        : t({ id: 'gizmo.selected', message: `${name}. Arrow keys move it.`, comment: 'A selected layer' })
    : target.locked
      ? t({
          id: 'gizmo.select.locked',
          message: `Select ${layer}, locked`,
          comment: '{layer} is the layer’s name',
        })
      : t({ id: 'gizmo.select', message: `Select ${layer}`, comment: '{layer} is the layer’s name' });
  /** Typing its words: on the canvas for text, in the inspector's first field for a title template. */
  const editWords = () => {
    if (target.locked) return;
    if (target.label === 'text') useTextEdit.getState().begin(target.id);
    else if (target.label === 'scene') focusInspectorText();
  };
  return (
    <button
      ref={ref}
      type="button"
      // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on.
      tabIndex={0}
      data-layer={target.id}
      aria-label={label}
      aria-roledescription={
        selected
          ? t({ id: 'gizmo.role', message: 'layer', comment: 'Screen-reader role of a selected layer' })
          : undefined
      }
      className={cn(
        'gizmo',
        selected && 'is-selected',
        fixed ? selected && 'is-fixed' : 'is-movable',
        dragging && 'is-dragging',
      )}
      style={{ ...boxStyle(current.box, canvas, current.rotation), zIndex: lifted ? 1 : undefined }}
      onClick={(e) => {
        // Keyboard presses arrive as clicks; the bare video and the stage must not see either.
        e.stopPropagation();
        useSelection.getState().select(target.id);
      }}
      onKeyDown={(e) => {
        if (!selected || nudge.onKeyDown(e, current, opsFor)) return;
        // Delete, undo and the rest act on the project with the nudges in it.
        if (!MODIFIERS.includes(e.key)) nudge.flush();
        if (e.key === 'Escape') {
          e.preventDefault();
          if (!cancel()) useSelection.getState().select(null);
        } else if (e.key === 'Enter' && (target.label === 'text' || target.label === 'scene')) {
          e.preventDefault();
          editWords();
        } else if (e.key === ' ') {
          // A focused layer isn't a button to press again: Space plays, as it does everywhere else.
          e.preventDefault();
          const timeline = useProject.getState().snapshot?.project.timeline;
          if (timeline && videoDurationUs(timeline) > 0) usePlayback.getState().toggle();
        }
      }}
      onBlur={nudge.flush}
      onDoubleClick={(e) => {
        if ((target.label !== 'text' && target.label !== 'scene') || target.locked) return;
        e.stopPropagation();
        editWords();
      }}
      {...handlers}
      onPointerDown={(e) => {
        nudge.flush();
        handlers.onPointerDown(e);
      }}
    >
      {selected && !playing && !fixed && <GizmoHandles sides={target.label === 'text'} compact={compact} />}
    </button>
  );
}
