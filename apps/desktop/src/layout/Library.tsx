import { useLingui } from '@lingui/react/macro';
import type { KeyboardEvent } from 'react';
import { CaptionsPanel } from '../features/captions/CaptionsPanel';
import { MediaList } from '../features/media/MediaList';
import { useImportMedia } from '../features/media/useImportMedia';
import { useCaptionsView } from '../state/captionsView';
import { useUi, type LibraryTab } from '../state/ui';
import { Icon } from './Icon';

const TABS: LibraryTab[] = ['clips', 'music', 'sounds', 'images'];

/**
 * The library panel (9.16): the Library and Captions view tabs, Add files, and the filter pills Clips,
 * Music, Sounds and Images over the files of each (yours first, then Klipolo's pack). The pills scroll sideways in a
 * narrow panel, with a fade at the right edge. The arrow keys move between them. Its Captions tab shows the Captions
 * view in its place.
 */
export function Library() {
  const { t } = useLingui();
  const tab = useUi((s) => s.libraryTab);
  const setTab = useUi((s) => s.setLibraryTab);
  const importMedia = useImportMedia();
  const captions = useCaptionsView((s) => s.open);
  const setCaptions = useCaptionsView((s) => s.setOpen);
  const names: Record<LibraryTab, string> = {
    clips: t({ id: 'library.tab.clips', message: 'Clips' }),
    music: t({ id: 'library.tab.music', message: 'Music' }),
    sounds: t({ id: 'library.tab.sounds', message: 'Sounds' }),
    images: t({ id: 'library.tab.images', message: 'Images' }),
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = TABS[(TABS.indexOf(tab) + step + TABS.length) % TABS.length];
    setTab(next);
    e.currentTarget.querySelector<HTMLElement>(`[data-tab="${next}"]`)?.focus();
  };
  const libraryLabel = t({ id: 'library.label', message: 'Library' });
  const captionsLabel = t({ id: 'library.captions', message: 'Captions' });
  return (
    <aside
      className="panel library"
      aria-label={captions ? captionsLabel : libraryLabel}
      data-view={captions ? 'captions' : 'library'}
      data-tab={tab}
    >
      <div className="panel-head">
        <div className="view-tabs" role="tablist">
          <button className="view-tab" role="tab" aria-selected={!captions} onClick={() => setCaptions(false)}>
            {t({ id: 'library.view', message: 'Library' })}
          </button>
          <button className="view-tab" role="tab" aria-selected={captions} onClick={() => setCaptions(true)}>
            {captionsLabel}
          </button>
        </div>
        <button
          className="icon-btn"
          aria-label={t({ id: 'library.addFiles', message: 'Add files' })}
          title={t({ id: 'library.addFiles.tip', message: 'Add files (Command O)' })}
          onClick={() => void importMedia()}
        >
          <Icon name="plus" />
        </button>
      </div>
      {captions ? (
        <CaptionsPanel />
      ) : (
        <>
          <div
            className="pills lib-pills"
            role="tablist"
            aria-label={t({ id: 'library.show', message: 'Show' })}
            onKeyDown={onKeyDown}
          >
            {TABS.map((name) => (
              <button
                key={name}
                className="pill"
                role="tab"
                data-tab={name}
                id={`library-tab-${name}`}
                aria-selected={tab === name}
                aria-controls="library-list"
                tabIndex={tab === name ? 0 : -1}
                onClick={() => setTab(name)}
              >
                {names[name]}
              </button>
            ))}
          </div>
          <div className="lib-scroll" role="tabpanel" id="library-list" aria-labelledby={`library-tab-${tab}`}>
            <MediaList tab={tab} />
          </div>
        </>
      )}
    </aside>
  );
}
