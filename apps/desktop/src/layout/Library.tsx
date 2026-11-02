import { useLingui } from '@lingui/react/macro';
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { SearchField } from '../design/SearchField';
import { SpriteIcon } from '../design/SpriteIcon';
import { CaptionsPanel } from '../features/captions/CaptionsPanel';
import { useShownFormat } from '../features/editor/formats';
import { Inspector } from '../features/inspector/Inspector';
import { LooksPanel } from '../features/looks/LooksPanel';
import { ClipRail } from '../features/media/ClipRail';
import { MediaList } from '../features/media/MediaList';
import { stopPreview } from '../features/media/preview';
import { useImportMedia } from '../features/media/useImportMedia';
import { useCaptionsView, type LeftView } from '../state/captionsView';
import { useLibraryPanel } from '../state/libraryPanel';
import { recordYourScreen } from '../state/recorder';
import { useUi, type LibraryTab } from '../state/ui';
import { useCardAside, useColumnCard, useLibraryKeepsItsPicks } from './columnCard';
import { Icon } from './Icon';

const TABS: LibraryTab[] = ['clips', 'music', 'sounds', 'images'];

/** The library's body, which its rail's Open library opens. */
export const LIBRARY_BODY_ID = 'library-body';

/** The inspector card in the column, in the format the preview shows (its own render, so the library's isn't asked). */
function ColumnInspector() {
  return <Inspector format={useShownFormat()} />;
}

/**
 * The library panel: the Library, Captions and Looks view tabs, Add files, Record your screen (the
 * Record dialog, as from the start screen and Shift Command R), a search over the tab shown, and the filter pills
 * Clips, Music, Sounds and Images over the files of each (yours first, then Klipolo's pack). The search keeps its words
 * from tab to tab. The pills scroll sideways in a narrow panel, with a fade at the right edge. The arrow keys move
 * between them. Its Captions tab shows the Captions view in its place, and its Looks tab the looks
 * (`LooksPanel`). Its head's Collapse library, Command B (or Control Command S), or a double click on its edge folds it
 * to its rail (`useLibraryPanel`): Open library (or the edge) opens it again, Add files is kept, and the clips'
 * thumbnails do what their rows do (`ClipRail`). Folded, it stays mounted (`hidden` and `inert`), and a fold with the
 * keyboard inside puts it on the rail.
 *
 * While something is selected the column shows its inspector card instead (`useColumnCard`), so the preview never
 * changes size on a click: the card fills the panel, and the library stays mounted under it, hidden and `inert`, with
 * its view, tab, search and scroll kept for when the card goes, and a sound heard from a row stops. A folded library
 * keeps its rail: the card lies over the preview's left edge instead, at the open library's width. A card that leaves
 * with the keyboard in it (Close, Escape, Back to library) leaves the keyboard on the panel, where Space still plays
 * and Tab goes on into the library. A pick made from the library itself (adding a file, a caption or a look) keeps the
 * library (`useLibraryKeepsItsPicks`); over the Looks opened from a clip's card, Back to settings brings the card back.
 */
export function Library() {
  const { t } = useLingui();
  const tab = useUi((s) => s.libraryTab);
  const setTab = useUi((s) => s.setLibraryTab);
  const importMedia = useImportMedia();
  const view = useCaptionsView((s) => s.view);
  const setView = useCaptionsView((s) => s.setView);
  const [query, setQuery] = useState('');
  const collapsed = useLibraryPanel((s) => s.collapsed);
  const card = useColumnCard();
  const cardAside = useCardAside();
  const panel = useRef<HTMLElement>(null);
  /** The keyboard was in the card last: when the card goes, it stays on the panel. */
  const inCard = useRef(false);
  useLibraryKeepsItsPicks(panel);
  // A sound being heard from a row stops as the card covers its row.
  useEffect(() => {
    if (card) stopPreview();
  }, [card]);
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
  useLayoutEffect(() => {
    if (card || !inCard.current) return;
    inCard.current = false;
    const active = document.activeElement;
    if (!active || active === document.body) panel.current?.focus({ preventScroll: true });
  }, [card]);
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
  const looksLabel = t({ id: 'library.looks', message: 'Looks', comment: 'The left panel’s view of colour looks' });
  const viewLabels = { library: libraryLabel, captions: captionsLabel, looks: looksLabel };
  const viewTab = (to: LeftView, name: string) => (
    <button tabIndex={0} className="view-tab" role="tab" aria-selected={view === to} onClick={() => setView(to)}>
      {name}
    </button>
  );
  return (
    <aside
      ref={panel}
      className="panel library"
      aria-label={viewLabels[view]}
      data-view={view}
      data-tab={tab}
      data-collapsed={collapsed}
      data-card={card || undefined}
      // Focusable from code only: where the keyboard waits once the card goes (Space plays from here).
      tabIndex={-1}
      onFocus={(e) => (inCard.current = e.target instanceof Element && e.target.closest('.inspector') !== null)}
    >
      {/* Its edge: a double click folds it, or opens it again from the rail. */}
      <div
        aria-hidden
        data-library-edge
        className="library-edge"
        onDoubleClick={() => useLibraryPanel.getState().toggle()}
      />
      {collapsed && (
        <div className="library-rail" hidden={card} inert={card}>
          <div className="rail-actions">
            {/* WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable. */}
            <button
              ref={rail}
              type="button"
              tabIndex={0}
              className="icon-btn"
              aria-label={t({ id: 'library.open', message: 'Open library' })}
              title={t({ id: 'library.open.tip', message: 'Open library (Command B)' })}
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
              tabIndex={0}
              className="icon-btn"
              aria-label={t({ id: 'library.addFiles', message: 'Add files' })}
              title={t({ id: 'library.addFiles.tip', message: 'Add files (Command O)' })}
              onClick={() => void importMedia()}
            >
              <Icon name="plus" />
            </button>
          </div>
          <ClipRail />
        </div>
      )}
      <div
        id={LIBRARY_BODY_ID}
        ref={body}
        className="library-body"
        hidden={collapsed}
        inert={collapsed || card}
        // Under the card it stays laid out, so its scroll is kept, but isn't drawn.
        style={card ? { visibility: 'hidden' } : undefined}
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
            {viewTab('library', t({ id: 'library.view', message: 'Library' }))}
            {viewTab('captions', captionsLabel)}
            {viewTab('looks', looksLabel)}
          </div>
          {/* WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable. */}
          <button
            type="button"
            tabIndex={0}
            className="icon-btn"
            aria-label={t({ id: 'library.addFiles', message: 'Add files' })}
            title={t({ id: 'library.addFiles.tip', message: 'Add files (Command O)' })}
            onClick={() => void importMedia()}
          >
            <Icon name="plus" />
          </button>
          <button
            type="button"
            tabIndex={0}
            className="icon-btn"
            aria-label={t({ id: 'library.record', message: 'Record your screen' })}
            title={t({ id: 'library.record.tip', message: 'Record your screen (Shift Command R)' })}
            onClick={recordYourScreen}
          >
            <Icon name="record" />
          </button>
          <button
            type="button"
            tabIndex={0}
            className="icon-btn"
            aria-label={t({ id: 'library.collapse', message: 'Collapse library' })}
            title={t({ id: 'library.collapse.tip', message: 'Collapse library (Command B)' })}
            aria-expanded
            aria-controls={LIBRARY_BODY_ID}
            onClick={() => useLibraryPanel.getState().setCollapsed(true)}
          >
            <Icon name="collapse" className="is-mirrored" />
          </button>
        </div>
        {view === 'captions' ? (
          <CaptionsPanel />
        ) : view === 'looks' ? (
          <>
            {/* Opened from a clip's card (its Look): the way back to that card while a look is chosen. */}
            {cardAside && (
              <button
                type="button"
                // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
                tabIndex={0}
                className="btn btn-secondary btn-sm lib-back-card"
                onClick={() => useLibraryPanel.getState().showCard()}
              >
                {t({ id: 'library.backToSettings', message: 'Back to settings' })}
              </button>
            )}
            <LooksPanel />
          </>
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
      {card && <ColumnInspector />}
    </aside>
  );
}
