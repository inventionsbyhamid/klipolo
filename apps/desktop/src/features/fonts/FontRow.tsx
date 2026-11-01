import { plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { memo, useEffect, useState, type CSSProperties } from 'react';
import type { FontRow as FontRowData } from '../../bindings/FontRow';
import { cn } from '../../design/cn';
import { Tooltip } from '../../design/Tooltip';
import { useIntlLocale } from '../../i18n/react';
import { ensurePreview, faceCss, faceLoaded, previewFamily, useFace, type FaceKey } from '../../lib/fonts';
import { formatDownloadSize, formatPercent } from '../../lib/format';
import { useFonts } from '../../state/fonts';
import { ROW_HEIGHT } from './useWindow';

/** How long a row stays in view before its face or preview is asked for: a row flown past asks nothing. */
const PREVIEW_SETTLE_MS = 150;

/** The face a pick of `row` draws with. */
const faceOf = (row: FontRowData): FaceKey => ({
  font: { family: row.family, source: row.source },
  weight: row.snapped[0],
  italic: row.snapped[1],
});

/**
 * A bundled or downloaded family's name in its own face, at the weight a pick draws: the engine's face, over IPC
 * (`useFace`, once per face). Only rows in view mount it, and a row asks only once it has stayed in view a moment: a
 * row flown past asks for nothing and adds no face to the page. A face asked for before draws at once.
 */
function OwnFace({ row }: { row: FontRowData }) {
  const known = faceLoaded(faceOf(row));
  const [settled, setSettled] = useState(known);
  useEffect(() => {
    if (settled) return;
    const wait = setTimeout(() => setSettled(true), PREVIEW_SETTLE_MS);
    return () => clearTimeout(wait);
  }, [settled]);
  return settled || known ? <FaceName row={row} /> : <span>{row.family}</span>;
}

function FaceName({ row }: { row: FontRowData }) {
  const face = faceOf(row);
  useFace(face);
  return <span style={faceCss(face)}>{row.family}</span>;
}

/** CSS widths by OpenType width class (1 ultra-condensed to 9 ultra-expanded). */
const STRETCH = ['', '50%', '62.5%', '75%', '87.5%', '100%', '112.5%', '125%', '150%', '200%'];

/**
 * An installed family's name in its own face, asked of the system by its name there and its width (`local()`): no
 * bytes cross IPC for a row in a list.
 */
function LocalFace({ row }: { row: FontRowData }) {
  const name = row.systemName ?? row.family;
  const style: CSSProperties = {
    fontFamily: `"${name.replace(/["\\]/g, '\\$&')}", sans-serif`,
    fontStretch: STRETCH[row.stretch] || '100%',
  };
  return <span style={style}>{row.family}</span>;
}

/**
 * A Google family not downloaded: its name in the interface font, and, once the person has confirmed Google Fonts
 * (and while it is on), in Google's small preview of it, asked once the row has stayed in view a moment. Never
 * before.
 */
function GoogleName({ family }: { family: string }) {
  const previews = useFonts((s) => s.googleFonts && s.googleConsented);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (!previews) return;
    let live = true;
    const wait = setTimeout(() => {
      void ensurePreview(family).then((ok) => live && setReady(ok));
    }, PREVIEW_SETTLE_MS);
    return () => {
      live = false;
      clearTimeout(wait);
    };
  }, [family, previews]);
  const style: CSSProperties | undefined =
    previews && ready ? { fontFamily: `"${previewFamily(family)}", var(--font-sans)` } : undefined;
  return <span style={style}>{family}</span>;
}

/**
 * One family in the font menu (`role="option"`): its name drawn in its own face, how many weights it has, and, for a
 * family installed on this computer, "This Mac only" (a project using it needs it on any other computer too). A Google
 * family not downloaded yet says "Download ({size})"; while it downloads, a bar and its percent. Its callbacks take
 * its option's number, so the list hands every row the same two (`FontRow`, memoised).
 */
function FontRowView({
  row,
  n,
  id,
  active,
  posinset,
  setsize,
  top,
  onPick,
  onHover,
}: {
  row: FontRowData;
  /** Its option's number, which `onPick` and `onHover` are called with. */
  n: number;
  id: string;
  active: boolean;
  /** Its place among all the list's options, and how many there are (the list is windowed). */
  posinset: number;
  setsize: number;
  /** Where the windowed list puts it (px). */
  top: number;
  onPick: (n: number) => void;
  onHover: (n: number) => void;
}) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const installed = row.state === 'installed';
  const download = row.state === 'download';
  const progress = useFonts((s) => (download ? s.downloads[row.family] : undefined));
  // Weights, not faces: Montserrat's 18 files are 9 weights, upright and italic.
  const count = new Set(row.faces.map(([weight]) => weight)).size;
  const size = download && row.sizeBytes !== null ? formatDownloadSize(locale, row.sizeBytes) : null;
  const fraction = progress && progress.total > 0 ? Math.min(1, progress.done / progress.total) : 0;
  const percent = formatPercent(locale, fraction);
  return (
    <div
      id={id}
      role="option"
      aria-selected={active}
      aria-posinset={posinset}
      aria-setsize={setsize}
      onClick={() => onPick(n)}
      onPointerEnter={() => onHover(n)}
      style={{ top, height: ROW_HEIGHT }}
      className={cn('font-row', active && 'active')}
    >
      <span className="font-row-name" title={row.family}>
        {installed ? <LocalFace row={row} /> : download ? <GoogleName family={row.family} /> : <OwnFace row={row} />}
      </span>
      {row.tooLarge && (
        <span className="font-row-meta">
          {t({
            id: 'fontMenu.tooLarge',
            message: 'Too large to preview',
            comment: 'An installed font file too big to load for previews; the video still uses it',
          })}
        </span>
      )}
      {row.defaultWeightOnly && (
        <span className="font-row-meta">
          {t({
            id: 'fontMenu.regularOnly',
            message: 'Only its regular weight for now',
            comment: 'An installed variable font: only its default weight draws for now',
          })}
        </span>
      )}
      {/* While it downloads, the bar and its percent take this place: the name stays readable. */}
      {!progress && (
        <span className="font-row-meta">
          {t({
            id: 'fontMenu.weights',
            message: plural(count, { one: '# weight', other: '# weights' }),
            comment: 'How many weights (Regular, Bold…) a font family has',
          })}
        </span>
      )}
      {progress ? (
        <span className="font-row-meta font-row-progress">
          <span aria-hidden className="font-bar">
            <span style={{ width: `${fraction * 100}%` }} />
          </span>
          {t({
            id: 'fontMenu.downloading',
            message: `Downloading… ${percent}`,
            comment: 'A Google font downloading after it was picked, and how far it has come (“40 %”)',
          })}
        </span>
      ) : (
        size !== null && (
          <span className="font-row-meta">
            {t({
              id: 'fontMenu.download',
              message: `Download (${size})`,
              comment: 'A Google font not on this computer yet, and how much picking it downloads (“2.9 MB”)',
            })}
          </span>
        )
      )}
      {installed && (
        <Tooltip
          label={t({
            id: 'fontMenu.thisMacOnly.tip',
            message: 'Installed on this Mac. Another computer needs it installed too.',
          })}
        >
          <span className="font-row-tag">
            {t({
              id: 'fontMenu.thisMacOnly',
              message: 'This Mac only',
              comment: 'Tag on a font installed on this computer, not included with Klipolo',
            })}
          </span>
        </Tooltip>
      )}
    </div>
  );
}

/** Memoised: a hover or a scroll renders only the rows whose props it changes. */
export const FontRow = memo(FontRowView);

/** An option whose row hasn't arrived yet (a browse's window): its place, and a quiet bar. */
export function PendingRow({
  id,
  active,
  position: [posinset, setsize],
  top,
}: {
  id: string;
  active: boolean;
  position: [number, number];
  top: number;
}) {
  const { t } = useLingui();
  return (
    <div
      id={id}
      role="option"
      aria-selected={active}
      aria-posinset={posinset}
      aria-setsize={setsize}
      aria-busy
      aria-label={t({ id: 'fontMenu.loading', message: 'Loading…', comment: 'A font’s row, still on its way' })}
      style={{ top, height: ROW_HEIGHT }}
      className={cn('font-row', active && 'active')}
    >
      <span aria-hidden className="font-row-pending" />
    </div>
  );
}
