import type { FormatKind } from '../../bindings/FormatKind';
import { useProject } from '../../state/project';
import { useUi } from '../../state/ui';
import { CaptionWords } from './CaptionWords';
import { StyleCards } from './StyleCards';
import './captions.css';

/** The format the preview shows: the one picked, else the project's primary one. */
function usePreviewedFormat(): FormatKind {
  const picked = useUi((s) => s.previewFormat);
  const primary = useProject((s) => s.snapshot?.project.settings.primaryFormat ?? 'landscape');
  return picked ?? primary;
}

/**
 * The Captions view of the left panel: the style cards, then the words of the format previewed, in one
 * scrolling column. Its tab in the panel's header is its title.
 */
export function CaptionsPanel() {
  const format = usePreviewedFormat();
  const open = useProject((s) => s.snapshot !== null);
  if (!open) return null;
  return (
    <div className="cap-scroll">
      <StyleCards />
      <CaptionWords format={format} />
    </div>
  );
}
