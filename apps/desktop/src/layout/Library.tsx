import { CaptionsPanel } from '../features/captions/CaptionsPanel';
import { useImportMedia } from '../features/media/useImportMedia';
import type { Asset } from '../bindings/Asset';
import { formatTime } from '../lib/time';
import { useCaptionsView } from '../state/captionsView';
import { useProject } from '../state/project';
import { Icon } from './Icon';

/**
 * The library panel: the project's clips, in the order they came in, and Add files. The other tabs come later. Its
 * Captions tab shows the Captions view in its place (9.17).
 */
export function Library() {
  const assets = useProject((s) => s.snapshot?.project.assets);
  const clips = assets?.filter((a) => a.kind === 'video');
  const importMedia = useImportMedia();
  const captions = useCaptionsView((s) => s.open);
  const setCaptions = useCaptionsView((s) => s.setOpen);
  return (
    <aside className="panel library" aria-label={captions ? 'Captions' : 'Library'} data-view={captions ? 'captions' : 'library'}>
      <div className="panel-head">
        <div className="view-tabs" role="tablist">
          <button className="view-tab" role="tab" aria-selected={!captions} onClick={() => setCaptions(false)}>
            Library
          </button>
          <button className="view-tab" role="tab" aria-selected={captions} onClick={() => setCaptions(true)}>
            Captions
          </button>
        </div>
        <button
          className="icon-btn"
          aria-label="Add files"
          title="Add files (Command O)"
          onClick={() => void importMedia()}
        >
          <Icon name="plus" />
        </button>
      </div>
      {captions ? (
        <CaptionsPanel />
      ) : (
        <LibraryBody clips={clips} />
      )}
    </aside>
  );
}

/** The Library view: its filter pills and the clips. */
function LibraryBody({ clips }: { clips: Asset[] | undefined }) {
  return (
    <>
      <div className="pills lib-pills" role="tablist" aria-label="Show">
        <button className="pill" role="tab" aria-selected="true">
          Clips
        </button>
        {['Music', 'Sounds', 'Images'].map((label) => (
          <button key={label} className="pill" role="tab" aria-selected="false" disabled>
            {label}
          </button>
        ))}
      </div>
      <div className="lib-scroll">
        {clips && clips.length > 0 ? (
          clips.map((clip) => (
            <div key={clip.id} className="row">
              <div className="art-clip" aria-hidden>
                <Icon name="film" />
              </div>
              <div className="row-text">
                <span className="row-title" title={clip.name}>
                  {clip.name}
                </span>
                <span className="row-meta">{formatTime(clip.durationUs)}</span>
              </div>
            </div>
          ))
        ) : (
          <div className="lib-empty">
            <p className="lib-empty-title">No clips yet</p>
            <p>Clips you add show up here. After analysis, Klipolo lists the best moments under them.</p>
          </div>
        )}
      </div>
    </>
  );
}
