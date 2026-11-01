import { useLingui } from '@lingui/react/macro';
import { useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { SearchField } from '../design/SearchField';
import { SpriteIcon } from '../design/SpriteIcon';
import { CaptionsPanel } from '../features/captions/CaptionsPanel';
import { MediaList } from '../features/media/MediaList';
import { useImportMedia } from '../features/media/useImportMedia';
import { useCaptionsView } from '../state/captionsView';
import { useLibraryPanel } from '../state/libraryPanel';
import { useUi, type LibraryTab } from '../state/ui';
import { Icon } from './Icon';

const TABS: LibraryTab[] = ['clips', 'music', 'sounds', 'images'];

/** The library's body, which its rail's Open library opens. */
export const LIBRARY_BODY_ID = 'library-body';

/**
 * The library panel (9.16): the Library and Captions view tabs, Add files, a search over the tab shown, and
 * the filter pills Clips, Music, Sounds and Images over the files of each (yours first, then Klipolo's pack). The
 * search keeps its words from tab to tab. The pills scroll sideways in a narrow panel, with a fade at the right edge.
 * The arrow keys move between them. Its Captions tab shows the Captions view in its place. Control
 * Command S folds it to its rail (`useLibraryPanel`), whose Open library opens it again; folded, it stays mounted
 * (`hidden` and `inert`), and a fold with the keyboard inside puts it on the rail.
 */
export function Library() {
  const { t } = useLingui();
  const tab = useUi((s) => s.libraryTab);
  const setTab = useUi((s) => s.setLibraryTab);
  const importMedia = useImportMedia();
  const captions = useCaptionsView((s) => s.open);
  const setCaptions = useCaptionsView((s) => s.setOpen);
  const [query, setQuery] = useState('');
  const collapsed = useLibraryPanel((s) => s.collapsed);
  const rail = useRef<HTMLButtonElement>(null);
  const body = useRef<HTMLDivElement>(null);
  /** The keyboard was in the library as it folded: it goes to the rail. */
  const inside = useRef(false);
  /** Opened from the rail: the keyboard goes to the view's tab once the body shows. */
  const wantFocus = useRef(false);
  useLayoutEffect(() => {
    if (collapsed && inside.current) rail.current?.focus();
    if (!collapsed && wantFocus.current) {
      wantFocus.current = false;
      body.current?.querySelector<HTMLElement>('.view-tab[aria-selected="true"]')?.focus();
    }
  }, [collapsed]);
  const names: Record<LibraryTab, string> = {
    clips: t({ id: 'library.tab.clips', message: 'Clips' }),
    music: t({ id: 'library.tab.music', message: 'Music' }),
    sounds: t({ id: 'library.tab.sounds', message: 'Sounds' }),
    images: t({ id: 'library.tab.images', message: 'Images' }),
  };
  const searchLabels: Record<LibraryTab, string> = {
    clips: t({ id: 'library.search.clips', message: 'Search clips' }),
    music: t({ id: 'library.search.music', message: 'Search music' }),
    sounds: t({ id: 'library.search.sounds', message: 'Search sounds' }),
    images: t({ id: 'library.search.images', message: 'Search images' }),
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
      data-collapsed={collapsed}
    >
      {collapsed && (
        <div className="library-rail">
          <button
            ref={rail}
            type="button"
            className="icon-btn"
            aria-label={t({ id: 'library.open', message: 'Open library' })}
            title={t({ id: 'library.open.tip', message: 'Open library (Control Command S)' })}
            aria-expanded={false}
            aria-controls={LIBRARY_BODY_ID}
            onClick={() => {
              wantFocus.current = true;
              useLibraryPanel.getState().setCollapsed(false);
            }}
          >
            <SpriteIcon name="expand-panel" className="is-mirrored" />
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label={t({ id: 'library.addFiles', message: 'Add files' })}
            title={t({ id: 'library.addFiles.tip', message: 'Add files (Command O)' })}
            onClick={() => void importMedia()}
          >
            <Icon name="plus" />
          </button>
        </div>
      )}
      <div
        id={LIBRARY_BODY_ID}
        ref={body}
        className="library-body"
        hidden={collapsed}
        inert={collapsed}
        onFocus={() => (inside.current = true)}
        onBlur={(e) => {
          const left = e.target;
          if (e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
          // Checked once the event is over: a control still focused only lost the window's focus (Command Tab).
          queueMicrotask(() => {
            if (left.isConnected && document.activeElement !== left) inside.current = false;
          });
        }}
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
            <SearchField className="lib-search" value={query} onChange={setQuery} label={searchLabels[tab]} />
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
              <MediaList tab={tab} query={query} />
            </div>
          </>
        )}
      </div>
    </aside>
  );
}
