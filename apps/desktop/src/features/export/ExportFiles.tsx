import { useShallow } from 'zustand/react/shallow';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatBytes } from '../../lib/format';
import { openExported, revealExported, useExport } from '../../state/export';
import { CAPTION_KINDS } from './CaptionFileChoice';

/** "SRT captions", "WebVTT captions": a captions file by its type, from its name. */
function captionsKind(name: string): string {
  const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
  const kind = CAPTION_KINDS.find((k) => k.kind === ext)?.name ?? ext.toUpperCase();
  return `${kind} captions`;
}

/**
 * The files in place, one row each: a small frame of its shape, its name as written (a taken
 * name gets " 2"), its pixel size and size on the disk, and Open. A captions file saved beside a video gets a row too,
 * with Show in Finder.
 */
export function ExportFiles() {
  const locale = useIntlLocale();
  const { files, ready, written, captions } = useExport(
    useShallow((s) => ({ files: s.files, ready: s.ready, written: s.written, captions: s.captionFiles })),
  );
  const rows = files.map((file, index) => ({ file, index })).filter(({ index }) => ready.includes(index));
  if (rows.length === 0 && captions.length === 0) return null;
  return (
    <ul className="ex-files">
      {rows.map(({ file, index }) => {
        const bytes = written.find((w) => w.name === file.name)?.bytes;
        const size = `${file.width} x ${file.height}`;
        return (
          <li key={index} className="ex-file">
            <span className="ex-art" aria-hidden>
              <i className={`ex-frame-${file.format}`} />
            </span>
            <span className="ex-file-text">
              <span className="ex-file-name" id={`export-file-${index}`} title={file.name}>
                {file.name}
              </span>
              <span className="ex-meta">{bytes ? `${size}, ${formatBytes(locale, bytes)}` : size}</span>
            </span>
            <Button
              variant="ghost"
              size="sm"
              aria-describedby={`export-file-${index}`}
              onClick={() => void openExported(index)}
            >
              Open
            </Button>
          </li>
        );
      })}
      {captions.map(({ index, name }) => (
        <li key={`captions-${index}`} className="ex-file">
          <span className="ex-art ex-art-captions" aria-hidden>
            <Icon name="text" />
          </span>
          <span className="ex-file-text">
            <span className="ex-file-name" id={`export-file-${index}`} title={name}>
              {name}
            </span>
            <span className="ex-meta">{captionsKind(name)}</span>
          </span>
          <Button
            variant="ghost"
            size="sm"
            aria-describedby={`export-file-${index}`}
            onClick={() => void revealExported(index)}
          >
            Show in Finder
          </Button>
        </li>
      ))}
    </ul>
  );
}
