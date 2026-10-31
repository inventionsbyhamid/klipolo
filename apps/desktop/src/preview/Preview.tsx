import { useLingui } from '@lingui/react/macro';
import { AnalysisBanner } from '../features/analysis/AnalysisBanner';
import { RoughCaptionsNote } from '../features/analysis/RoughCaptionsNote';
import { FormatPills } from '../features/editor/FormatPills';
import { useShownFormat } from '../features/editor/formats';
import { Inspector, useInspected } from '../features/inspector/Inspector';
import { useImportMedia } from '../features/media/useImportMedia';
import { AddTitleMenu } from '../features/preview/AddTitleMenu';
import { Stage } from '../features/preview/Stage';
import { Icon } from '../layout/Icon';
import { useEngine } from '../state/engine';
import { useProject } from '../state/project';
import { useUi } from '../state/ui';

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
 * The preview panel: the format pills, Add title, the safe area switch, and the engine's frames fitted to
 * the panel in the chosen format with the layers there selectable and editable (`Stage`), under the analysis banner;
 * the inspector docks at the right while a layer it has fields for is selected, and the frame and banner move left for
 * it. With no clip yet, the drop zone.
 * `filesOver`: files from Finder are held over the window.
 */
export function Preview({ filesOver = false }: { filesOver?: boolean }) {
  const { t } = useLingui();
  const status = useEngine((s) => s.status);
  const message = useEngine((s) => s.message);
  const hasVideo = useProject((s) => (s.snapshot?.project.timeline.main.length ?? 0) > 0);
  const format = useShownFormat();
  const safeArea = useUi((s) => s.safeArea);
  const setSafeArea = useUi((s) => s.setSafeArea);
  const inspected = useInspected();
  const native = useEngine((s) => s.presenter === 'native' && !!s.info);
  const docked = hasVideo && inspected !== null;
  return (
    <main className={native && hasVideo ? 'panel stage has-hole' : 'panel stage'} aria-label="Preview">
      <div className="stage-head">
        <FormatPills />
        <span className="spacer" />
        {status !== 'unavailable' && message && <span className="stage-status">{message}</span>}
        <RoughCaptionsNote />
        <AddTitleMenu disabled={!hasVideo} />
        <button
          className="icon-btn"
          aria-label={t({ id: 'preview.safeArea', message: 'Show safe area' })}
          title={t({ id: 'preview.safeArea.tip', message: 'Safe area: where the apps leave a 9:16 video clear' })}
          aria-pressed={safeArea !== 'off'}
          onClick={() => setSafeArea(safeArea === 'off' ? 'all' : 'off')}
        >
          <Icon name="safe" />
        </button>
        <button className="icon-btn" aria-label="Full screen preview" title="Full screen (F)" disabled>
          <Icon name="full" />
        </button>
      </div>
      <div className={docked ? 'stage-body has-inspector' : 'stage-body'}>
        {hasVideo && <AnalysisBanner />}
        {hasVideo ? <Stage /> : <DropZone over={filesOver} />}
        {docked && <Inspector format={format} />}
      </div>
    </main>
  );
}
