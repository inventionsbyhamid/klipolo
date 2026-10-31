import { useShallow } from 'zustand/react/shallow';
import { exporter } from '../../bridge/exporter';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { formatBytes } from '../../lib/format';
import { useExport } from '../../state/export';

/**
 * The files in place, one row each: a small frame of its shape, its name as written (a taken
 * name gets " 2"), its pixel size and size on the disk, and Open.
 */
export function ExportFiles() {
  const locale = useIntlLocale();
  const { files, ready, written } = useExport(
    useShallow((s) => ({ files: s.files, ready: s.ready, written: s.written })),
  );
  const rows = files.map((file, index) => ({ file, index })).filter(({ index }) => ready.includes(index));
  if (rows.length === 0) return null;
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
              onClick={() => void exporter.open(index).catch(() => undefined)}
            >
              Open
            </Button>
          </li>
        );
      })}
    </ul>
  );
}
