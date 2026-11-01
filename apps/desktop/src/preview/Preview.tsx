import { useLingui } from '@lingui/react/macro';
import { useState } from 'react';
import { AnalysisBanner } from '../features/analysis/AnalysisBanner';
import { RoughCaptionsNote } from '../features/analysis/RoughCaptionsNote';
import { FormatPills } from '../features/editor/FormatPills';
import { useShownFormat } from '../features/editor/formats';
import { Inspector, useInspected } from '../features/inspector/Inspector';
import { useInspectedTransition } from '../features/inspector/TransitionInspector';
import { useImportMedia } from '../features/media/useImportMedia';
import { AddTitleMenu } from '../features/preview/AddTitleMenu';
import { SafeAreaMenu } from '../features/preview/SafeAreaMenu';
import { Stage } from '../features/preview/Stage';
import { Icon } from '../layout/Icon';
import { useDockHold } from '../state/dockHold';
import { useEngine } from '../state/engine';
import { useProject } from '../state/project';
import { useSelection } from '../state/selection';

/** The format the preview shows (it lives with the format switch). */
export { useShownFormat };

/**
 * An empty project's drop zone: drop videos anywhere on the window, or choose them (the one red), or
 * files are held over the window.
 */
function DropZone({ over }: { over: boolean }) {
  const importMedia = useImportMedia();
  return (
    <div className={over ? 'drop is-over' : 'drop'}>
      <div className="drop-art" aria-hidden>
        <b>
          <Icon name="plus" />
        </b>
      </div>
      <h1 className="drop-title">Drop videos here</h1>
      <p className="drop-text">MOV, MP4 or M4V. Add several at once and they line up in the order you drop them.</p>
      <div className="drop-actions">
        <button className="btn btn-primary btn-lg" onClick={() => void importMedia()}>
          Choose files
        </button>
        <span>
          or press <kbd>&#8984;</kbd> <kbd>O</kbd>
        </span>
      </div>
    </div>
  );
}

/**
 * Whether the card shows: `wanted`, except while a timeline press or drag, or a keyboard walk along the lanes, holds
 * the dock (`state/dockHold`); then what it was before, so the card doesn't come and go under the pointer or at every
 * bare cut. It follows `wanted` again once the hold ends.
 */
function useSteadyDock(wanted: boolean): boolean {
  const held = useDockHold((s) => s.press || s.walk);
  const [shown, setShown] = useState(wanted);
  if (!held && shown !== wanted) setShown(wanted);
  return held ? shown : wanted;
}

/**
 * The preview panel: the format pills, Add title, the safe area menu (9:16 only), Full screen (F; it
 * it works), and the engine's frames fitted to the panel in the chosen format with the layers there selectable and
 * editable (`Stage`), under the analysis banner; the inspector shows at the right under the banner while a main clip, a
 * transition or a layer it has fields for is selected (never mid-press or mid-walk on the timeline: `useSteadyDock`).
 * Selecting never resizes or moves the frame: the card goes in a slot of its own beside it, or over its right side when
 * a slot would cost the frame too much of its size (`stageLayout`). An empty slot says what goes there. With no clip
 * yet, the drop zone.
 * `filesOver`: files from Finder are held over the window.
 */
export function Preview({ filesOver = false }: { filesOver?: boolean }) {
  const { t } = useLingui();
  const status = useEngine((s) => s.status);
  const message = useEngine((s) => s.message);
  const hasVideo = useProject((s) => (s.snapshot?.project.timeline.main.length ?? 0) > 0);
  const format = useShownFormat();
  const inspected = useInspected();
  const transition = useInspectedTransition();
  const native = useEngine((s) => s.presenter === 'native' && !!s.info);
  const docked = useSteadyDock(inspected !== null || transition !== null) && hasVideo;
  const nothingSelected = useSelection((s) => s.selectedId === null);
  return (
    <main className={native && hasVideo ? 'panel stage has-hole' : 'panel stage'} aria-label="Preview">
      <div className="stage-head">
        <FormatPills />
        <span className="spacer" />
        {status !== 'unavailable' && message && <span className="stage-status">{message}</span>}
        <RoughCaptionsNote />
        <AddTitleMenu disabled={!hasVideo} />
        <SafeAreaMenu />
      </div>
      <div className={docked ? 'stage-body has-inspector' : 'stage-body'}>
        {hasVideo && <AnalysisBanner />}
        {/* Under the banner: the frame, and the inspector at its right (in the slot, or over the frame). */}
        <div className="stage-main">
          {hasVideo ? <Stage /> : <DropZone over={filesOver} />}
          {docked && <Inspector format={format} />}
          {/* Drawn in the slot only (app.css): over the frame it would cover the video. */}
          {hasVideo && !docked && nothingSelected && (
            <p className="stage-hint">
              {t({ id: 'preview.hint.select', message: 'Select something on the video to change it.' })}
            </p>
          )}
        </div>
      </div>
    </main>
  );
}
