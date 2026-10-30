import { useSession } from '../player/session';
import { formatTime } from '../player/time';
import { Icon } from './Icon';

/** The library panel: the open video as its one clip. Tabs, filters and adding files are placeholders. */
export function Library() {
  const project = useSession((s) => s.project);
  return (
    <aside className="panel library" aria-label="Library">
      <div className="panel-head">
        <div className="view-tabs" role="tablist">
          <button className="view-tab" role="tab" aria-selected="true">
            Library
          </button>
          <button className="view-tab" role="tab" aria-selected="false" disabled>
            Captions
          </button>
        </div>
        <button className="icon-btn" aria-label="Add files" title="Add files (Command O)" disabled>
          <Icon name="plus" />
        </button>
      </div>
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
        {project ? (
          <div className="row is-selected">
            <div className="art-clip" aria-hidden>
              <Icon name="film" />
            </div>
            <div className="row-text">
              <span className="row-title" title={project.name}>
                {project.name}
              </span>
              <span className="row-meta">{formatTime(project.durationUs)}</span>
            </div>
          </div>
        ) : (
          <div className="lib-empty">
            <p className="lib-empty-title">No clips yet</p>
            <p>Clips you add show up here. After analysis, Klipolo lists the best moments under them.</p>
          </div>
        )}
      </div>
    </aside>
  );
}
