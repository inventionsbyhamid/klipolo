import { useLingui } from '@lingui/react/macro';
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { BrowseAsk } from '../../bindings/BrowseAsk';
import type { Category } from '../../bindings/Category';
import type { FontRef } from '../../bindings/FontRef';
import type { FontRow as FontRowData } from '../../bindings/FontRow';
import type { MoreFace } from '../../bindings/MoreFace';
import { Popover, PopoverContent, PopoverTrigger } from '../../design/Popover';
import { faceCss, useFace } from '../../lib/fonts';
import { Icon } from '../../layout/Icon';
import { downloadFont, loadGoogleFonts, startFontsFeed, useFonts, useRecentFonts } from '../../state/fonts';
import { FontFilters } from './FontFilters';
import { FontList, optionId } from './FontList';
import { FontMenuNotes } from './FontMenuNotes';
import { GoogleConsent } from './GoogleConsent';
import { useBrowseListing, useQueryListing } from './listing';
import { useBrowse } from './useBrowse';
import { useProjectFonts } from './useProjectFonts';
import { useRestingHover } from './useRestingHover';
import { useFontRow, useFontSearch } from './useFontSearch';
import { useGooglePick } from './useGooglePick';
import { WeightMenu } from './WeightMenu';
import './fonts.css';

/** Letters of a script other than Latin: "Covers my text" starts on for such a text. */
const NON_LATIN = /[^\p{Script=Latin}\p{Script=Common}\p{Script=Inherited}]/u;

const fontOf = (row: FontRowData): FontRef => ({ family: row.family, source: row.source });

/**
 * The font menu, for a title's font: a searchable, windowed list of the 12 bundled families first, then the project's
 * and the recent picks, this computer's and Google's, in sections, from Rust's one ranked search. Browsing (no query)
 * is windowed in Rust too: the page gets each section's count and only the rows it draws. With Google Fonts off, no
 * family that needs a download is listed, and a line under the list says how to turn it on. Arrow keys and a hover
 * that rests a moment preview a font on the text (`onPreview`), Escape puts it back, Enter or a click picks it. A
 * pick's weight and style are already the face the family has nearest (`FontRow.snapped`). A Google family not
 * downloaded asks once, in place, then downloads, and is applied once it is here.
 */
export function FontMenu({
  label,
  value,
  weight,
  italic,
  text = '',
  onPick,
  onPreview,
}: {
  label: string;
  value: FontRef;
  weight: number;
  italic: boolean;
  /** The text the font is for: "Covers my text" starts on when it has non-Latin letters. */
  text?: string;
  onPick: (font: FontRef, weight: number, italic: boolean) => void;
  /** A font shown on the text for now, or null to put back what was there. */
  onPreview?: (font: FontRef | null, weight: number, italic: boolean) => void;
}) {
  const { t } = useLingui();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<Category | null>(null);
  const [covers, setCovers] = useState(() => NON_LATIN.test(text));
  const idPrefix = useId();
  const input = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const previewed = useRef(false);
  const picks = useRef(0);
  const typed = query.trim() !== '';
  const filter = {
    category,
    covers: covers && text ? text : null,
    want: [weight, italic] as [number, boolean],
  };
  const rows = useFontSearch(query, filter, open && typed);
  const project = useProjectFonts();
  const recent = useRecentFonts((s) => s.recent);
  const ask: BrowseAsk = { filter, project, recent, value };
  const browsed = useBrowse(ask, open && !typed);
  // The chosen family's own row, for its weights and how its name is drawn: one lookup, not a search.
  const valueRow = useFontRow(value, weight, italic);
  const google = useFonts((s) => s.googleFonts);
  useEffect(() => {
    if (!open) return;
    startFontsFeed();
    void loadGoogleFonts();
  }, [open]);
  // Rust leaves them out while Google Fonts is off; the menu never lists one either way.
  const shown = useMemo(() => (rows ?? []).filter((r) => google || r.state !== 'download'), [rows, google]);
  const queryListing = useQueryListing(shown);
  const browseListing = useBrowseListing(browsed.browse, browsed.rows);
  const listing = typed ? queryListing : browseListing;
  const answer = typed ? rows : browsed.browse;
  // A failure while the menu is closed is a toast, never kept in the closed popover.
  const openNow = useRef(false);
  useLayoutEffect(() => {
    openNow.current = open;
  });
  const googlePick = useGooglePick(() => openNow.current);
  // After a query, Rust's best match: the option whose row comes first in Rust's answer (the sections list "On this
  // Mac" before Google's, which Rust may rank higher). Before one, the chosen font's row.
  const best = useMemo(() => {
    const rank = new Map(shown.map((r, i) => [r, i]));
    let at = -1;
    let low = Infinity;
    for (let n = 0; n < queryListing.options; n++) {
      const r = rank.get(queryListing.row(n)!) ?? Infinity;
      if (r < low) [at, low] = [n, r];
    }
    return at;
  }, [shown, queryListing]);
  const start = typed ? best : (browsed.browse?.value ?? -1);
  const [moved, setMoved] = useState<{ answer: unknown; n: number; keyed: boolean } | null>(null);
  const mine = moved && moved.answer === answer ? moved : null;
  const active = mine ? mine.n : Math.min(start, listing.options - 1);
  // Kept in view when the keyboard moved it (or it is where the list opens); never for the pointer, which is on it.
  const revealed = active >= 0 && (mine?.keyed ?? true) ? listing.lineOf(active) : -1;
  // Back from the consent, the search field has the keyboard again.
  const asking = googlePick.asking;
  useEffect(() => {
    if (open && !asking) input.current?.focus();
  }, [asking, open]);

  const preview = (n: number) => {
    const row = listing.row(n);
    if (!row || row.state === 'download' || !onPreview) return;
    previewed.current = true;
    onPreview(fontOf(row), row.snapped[0], row.snapped[1]);
  };
  // The pointer's preview waits for it to rest on a row; the keys' shows at once.
  const hover = useRestingHover(preview);
  const show = (n: number, keyed: boolean) => {
    setMoved({ answer, n, keyed });
    hover.stop();
    if (keyed) preview(n);
    else hover.rest(n);
  };
  // A font applied after its download goes through the latest `onPick`: built from the text as it is
  // then, so edits made while it downloaded stay.
  const latestPick = useRef(onPick);
  useLayoutEffect(() => {
    latestPick.current = onPick;
  });
  const apply = (font: FontRef, w: number, i: boolean) => {
    hover.stop();
    previewed.current = false;
    setOpen(false);
    latestPick.current(font, w, i);
  };
  const pick = (n: number) => {
    const row = listing.row(n);
    if (!row) return;
    const serial = ++picks.current;
    if (row.state !== 'download') {
      apply(fontOf(row), row.snapped[0], row.snapped[1]);
      return;
    }
    // Applied once it is here, unless the person picked something else meanwhile.
    const [w, i] = row.snapped;
    // A family already downloading is joined, never fetched twice: this pick takes over its apply.
    void googlePick.pick({
      family: row.family,
      bytes: row.sizeBytes ?? 0,
      run: () => downloadFont(row.family, null, w, i),
      apply: (font) => {
        if (picks.current === serial) apply(font, w, i);
      },
    });
  };
  // "Download {weight} ({size})" of a family downloaded a weight at a time: applied once it is here.
  const downloadFace = (face: MoreFace) => {
    const serial = ++picks.current;
    const version = value.source.type === 'google' ? value.source.version : null;
    void googlePick.pick({
      family: value.family,
      bytes: face.bytes,
      run: () => downloadFont(value.family, version, face.weight, face.italic),
      apply: (font) => {
        if (picks.current === serial) apply(font, face.weight, face.italic);
      },
    });
  };
  const onOpenChange = (next: boolean) => {
    hover.stop();
    setOpen(next);
    if (next) {
      setQuery('');
      setMoved(null);
      googlePick.clearError();
    } else {
      googlePick.cancelAsk();
      if (previewed.current) {
        previewed.current = false;
        onPreview?.(null, weight, italic);
      }
    }
  };

  const googleOff = `${idPrefix}-google-off`;
  const family = value.family;
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="select font-trigger"
          aria-label={t({
            id: 'fontMenu.trigger',
            message: `${label}: ${family}`,
            comment: '{label} is the setting (“Font”), {family} a font’s name',
          })}
        >
          <FontName font={valueRow ? fontOf(valueRow) : value} weight={weight} italic={italic} />
          <Icon name="chev-down" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="font-menu"
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          input.current?.focus();
        }}
        onEscapeKeyDown={(e) => {
          // In the consent, Escape goes back to the list, not out of the menu.
          if (!asking) return;
          e.preventDefault();
          googlePick.cancelAsk();
        }}
      >
        {asking ? (
          <GoogleConsent
            bytes={asking.bytes}
            onDownload={() => void googlePick.confirm()}
            onCancel={googlePick.cancelAsk}
          />
        ) : (
          <>
            <div className="font-search-row">
              <input
                ref={input}
                type="search"
                // ARIA 1.2's combobox: VoiceOver reads the active option the field points at (`aria-activedescendant`).
                role="combobox"
                aria-expanded
                aria-autocomplete="list"
                aria-label={t({ id: 'fontMenu.search', message: 'Search fonts' })}
                placeholder={t({ id: 'fontMenu.search', message: 'Search fonts' })}
                aria-controls={`${idPrefix}-list`}
                aria-activedescendant={active >= 0 ? optionId(idPrefix, active) : undefined}
                value={query}
                onChange={(e) => {
                  hover.stop();
                  setQuery(e.target.value);
                  setMoved(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                    e.preventDefault();
                    if (listing.options === 0) return;
                    const step = e.key === 'ArrowDown' ? 1 : -1;
                    show(Math.min(listing.options - 1, Math.max(0, active + step)), true);
                  } else if (e.key === 'Enter') {
                    e.preventDefault();
                    pick(active);
                  }
                }}
                className="input input-sm font-search"
              />
              <WeightMenu
                faces={valueRow?.faces.length ? valueRow.faces : [[weight, italic]]}
                weight={weight}
                italic={italic}
                onPick={(w, i) => apply(value, w, i)}
                // Never offered while Google Fonts is off: Rust would refuse it.
                more={google ? valueRow?.moreFaces : undefined}
                onDownload={downloadFace}
              />
            </div>
            <FontFilters category={category} covers={covers} onCategory={setCategory} onCovers={setCovers} />
            <div className="font-menu-body">
              {answer !== null && listing.options === 0 ? (
                <p className="font-note font-empty">
                  {typed
                    ? t({ id: 'fontMenu.empty', message: `No fonts match “${query}”.` })
                    : t({ id: 'fontMenu.emptyFilters', message: 'No fonts match these filters.' })}
                </p>
              ) : (
                <FontList
                  listing={listing}
                  idPrefix={idPrefix}
                  active={active}
                  activeItem={revealed}
                  label={t({ id: 'fontMenu.list', message: 'Fonts' })}
                  describedBy={google ? undefined : googleOff}
                  onPick={pick}
                  onHover={(n) => show(n, false)}
                  onWindow={typed ? undefined : browsed.want}
                  listRef={listRef}
                />
              )}
              <FontMenuNotes offId={googleOff} error={googlePick.error} />
            </div>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}

/** A family's name drawn in the face the engine would draw a text in it with, at that text's weight and style. */
function FontName({ font, weight, italic }: { font: FontRef; weight: number; italic: boolean }) {
  const face = { font, weight, italic };
  useFace(face);
  return (
    <span className="select-text" style={faceCss(face)}>
      {font.family}
    </span>
  );
}
