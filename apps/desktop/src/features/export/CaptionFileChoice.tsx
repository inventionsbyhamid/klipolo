import { type KeyboardEvent, useId } from 'react';
import type { CaptionFile } from '../../bindings/CaptionFile';
import { Toggle } from '../../design/Toggle';
import { useExport } from '../../state/export';

/** The file types, SRT first, by their own names (never translated). */
export const CAPTION_KINDS: { kind: CaptionFile; name: string }[] = [
  { kind: 'srt', name: 'SRT' },
  { kind: 'vtt', name: 'WebVTT' },
];

/**
 * "Also save captions as a file": beside the 16:9 video, its captions as an SRT or WebVTT file (SRT unless the person
 * picks WebVTT), for YouTube's subtitles. Its lines are the Clean style's, with the person's word fixes, whatever style
 * is burned in. Kept on this Mac for every project. The dialog shows it once some clip's words are heard and a 16:9
 * video is checked.
 */
export function CaptionFileChoice() {
  const id = useId();
  const { save, kind } = useExport((s) => s.captionFile);
  const setCaptionFile = useExport((s) => s.setCaptionFile);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const next = CAPTION_KINDS.find((k) => k.kind !== kind) ?? CAPTION_KINDS[0];
    setCaptionFile({ kind: next.kind });
    e.currentTarget.querySelector<HTMLElement>(`[data-kind="${next.kind}"]`)?.focus();
  };
  return (
    <div className="ex-captions">
      <span className="ex-switch">
        <Toggle id={id} checked={save} onChange={(on) => setCaptionFile({ save: on })} />
        <label htmlFor={id}>Also save captions as a file</label>
      </span>
      <div className="seg" role="radiogroup" aria-label="Captions file type" onKeyDown={onKeyDown}>
        {CAPTION_KINDS.map((k) => (
          <button
            key={k.kind}
            type="button"
            role="radio"
            aria-checked={kind === k.kind}
            tabIndex={kind === k.kind ? 0 : -1}
            data-kind={k.kind}
            onClick={() => setCaptionFile({ kind: k.kind })}
          >
            {k.name}
          </button>
        ))}
      </div>
    </div>
  );
}
