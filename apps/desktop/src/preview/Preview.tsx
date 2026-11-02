import { useLingui } from '@lingui/react/macro';
import { AnalysisBanner } from '../features/analysis/AnalysisBanner';
import { MissingFontsBanner } from '../features/fonts/MissingFontsBanner';
import { RoughCaptionsNote } from '../features/analysis/RoughCaptionsNote';
import { FormatPills } from '../features/editor/FormatPills';
import { useShownFormat } from '../features/editor/formats';
import { useImportMedia } from '../features/media/useImportMedia';
import { AddTitleMenu } from '../features/preview/AddTitleMenu';
import { SafeAreaMenu } from '../features/preview/SafeAreaMenu';
import { Stage } from '../features/preview/Stage';
import { EditFrames } from '../features/timeline/commands';
import { Icon } from '../layout/Icon';
import { useEngine } from '../state/engine';
import { useFullScreen } from '../state/fullScreen';
import { useProject } from '../state/project';
import { recordYourScreen } from '../state/recorder';

/** The format the preview shows (it lives with the format switch). */
export { useShownFormat };

/**
 * An empty project's drop zone: drop videos anywhere on the window, or choose them (the one yellow), or
 * record your screen (the Record dialog, as from the start screen). It turns paper while files are held over the
 * window.
 */
function DropZone({ over }: { over: boolean }) {
  const { t } = useLingui();
  const importMedia = useImportMedia();
  return (
    <div className={over ? 'drop is-over' : 'drop'}>
      <div className="drop-art" aria-hidden>
        <b>
          <Icon name="plus" />
        </b>
      </div>
      <h1 className="drop-title">{t({ id: 'preview.empty.title', message: 'Drop videos here' })}</h1>
      <p className="drop-text">
        {t({
          id: 'preview.empty.text',
          message: 'MOV, MP4 or M4V. Add several at once and they line up in the order you drop them.',
        })}
      </p>
      <div className="drop-actions">
        {/* WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable. */}
        <button type="button" tabIndex={0} className="btn btn-primary btn-lg" onClick={() => void importMedia()}>
          {t({ id: 'preview.empty.choose', message: 'Choose files' })}
        </button>
        <button type="button" tabIndex={0} className="btn btn-secondary btn-lg" onClick={recordYourScreen}>
          <Icon name="record" />
          {t({ id: 'preview.empty.record', message: 'Record your screen' })}
        </button>
        <span>
          or press <kbd>&#8984;</kbd> <kbd>O</kbd> to choose files
        </span>
      </div>
    </div>
  );
}

/**
 * The preview panel: the format pills, Add title, the safe area menu (9:16 only), Full screen (F; it
 * fills the screen with this panel, and Escape or the same button leaves), and the engine's frames fitted to the panel
 * in the chosen format with the layers there selectable and editable (`Stage`), under the analysis banner. Selecting
 * never resizes or moves the frame: the inspector card shows in the left column, over the library (`Library`). With no
 * clip yet, the drop zone. `filesOver`: files from Finder are held over the window.
 */
export function Preview({ filesOver = false }: { filesOver?: boolean }) {
  const { t } = useLingui();
  const status = useEngine((s) => s.status);
  const message = useEngine((s) => s.message);
  const hasVideo = useProject((s) => (s.snapshot?.project.timeline.main.length ?? 0) > 0);
  const native = useEngine((s) => s.presenter === 'native' && !!s.info);
  const full = useFullScreen((s) => s.on);
  return (
    // Focusable from code only: a project that opens puts the keyboard here, so Space plays (`useKeysOnEditor`).
    <main className={native && hasVideo ? 'panel stage has-hole' : 'panel stage'} aria-label="Preview" tabIndex={-1}>
      <div className="stage-head">
        <FormatPills />
        <span className="spacer" />
        {status !== 'unavailable' && message && <span className="stage-status">{message}</span>}
        <RoughCaptionsNote />
        <AddTitleMenu disabled={!hasVideo} />
        <SafeAreaMenu />
        {/* Off until there is a video to watch (focusable, so its tooltip says why). */}
        <button
          tabIndex={0}
          className="icon-btn"
          aria-label={
            full
              ? t({ id: 'preview.full.leave', message: 'Leave full screen' })
              : t({ id: 'preview.full', message: 'Full screen preview' })
          }
          title={
            full
              ? t({ id: 'preview.full.leave.tip', message: 'Leave full screen (Esc)' })
              : hasVideo
                ? t({ id: 'preview.full.tip', message: 'Full screen preview (F)' })
                : t({ id: 'preview.full.off', message: 'Add a video to watch it full screen' })
          }
          aria-disabled={(!hasVideo && !full) || undefined}
          onClick={() => {
            if (hasVideo || full) void useFullScreen.getState().toggle();
          }}
        >
          <Icon name={full ? 'full-exit' : 'full'} />
        </button>
      </div>
      <div className="stage-body">
        {hasVideo && <AnalysisBanner />}
        <MissingFontsBanner />
        {/* Under the banners: the frame. */}
        <div className="stage-main">
          {hasVideo ? <Stage /> : <DropZone over={filesOver} />}
          {/* A roll's, slip's or slide's two frames at the edit, while one is dragged. */}
          {hasVideo && <EditFrames />}
        </div>
      </div>
    </main>
  );
}
