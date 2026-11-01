import { useMemo } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Project } from '../../bindings/Project';
import { useLocale } from '../../i18n/react';
import type { Size } from '../../lib/geometry';
import { itemEndUs } from '../../lib/timeline';
import { usePlayback } from '../../state/playback';
import { selectionOf, useSelection } from '../../state/selection';
import { Gizmo } from './Gizmo';
import { gizmoTargets } from './gizmoTargets';
import { SafeArea } from './SafeArea';
import { SnapGuides } from './SnapGuides';
import { findText, useTextEdit } from './textEdit';
import { useEditEnds } from './textEditKeys';
import { TextEditor } from './TextEditor';

interface GizmoLayerProps {
  project: Project;
  format: FormatKind;
  playheadUs: number;
  canvas: Size;
}

/**
 * Hit boxes for everything pickable at the playhead, in the engine's draw order (later is on top), over whatever
 * draws the pixels, with the 9:16 safe area under them and snap guides over them. Boxes come from the TS layout
 * functions pinned to the engine (the layout parity test). A title being typed has the canvas editor over it instead
 * of its box. Every layer of a group selected on the timeline draws selected.
 */
export function GizmoLayer({ project, format, playheadUs, canvas }: GizmoLayerProps) {
  const selectedId = useSelection((s) => s.selectedId);
  const selectedIds = useSelection((s) => s.selectedIds);
  // A group selected on the timeline is selected here too: each of its layers at the playhead draws selected.
  const selection = useMemo(() => new Set(selectionOf({ selectedId, selectedIds })), [selectedId, selectedIds]);
  const playing = usePlayback((s) => s.playing);
  // The targets' names ("Missing media") are in the language shown.
  const locale = useLocale();
  const targets = useMemo(
    () => gizmoTargets(project, playheadUs, format),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- names follow the language shown
    [project, playheadUs, format, locale],
  );
  // The edit's identity only (its words change per key, which only the editor follows).
  const editingId = useTextEdit((s) => s.session?.id ?? null);
  const shownId = useTextEdit((s) => (s.session ?? s.closing)?.id ?? null);
  const draft = useTextEdit((s) => (s.session ?? s.closing)?.draft ?? null);
  useEditEnds(project);
  const edited = draft ?? (shownId === null ? null : (findText(project, shownId)?.item ?? null));
  // A new box always shows; a title only at the playhead (`playheadUs` is where the scene there starts).
  const showEditor =
    edited !== null && (draft !== null || (edited.startUs <= playheadUs && playheadUs < itemEndUs(edited)));
  // The selected layer and those drawn after it are lifted, keeping their order among themselves; a title being typed
  // is above them all.
  const selectedAt = targets.findIndex((t) => t.id === selectedId);
  return (
    <>
      <SafeArea format={format} />
      {targets.map((t, i) =>
        t.id === editingId ? null : (
          <Gizmo
            key={t.id}
            target={t}
            others={targets}
            selected={selection.has(t.id)}
            canvas={canvas}
            format={format}
            primary={project.settings.primaryFormat}
            playing={playing}
            lifted={selectedAt >= 0 && i >= selectedAt}
          />
        ),
      )}
      {showEditor && (
        <TextEditor
          key={edited.id}
          item={edited}
          editable={editingId === edited.id}
          canvas={canvas}
          format={format}
          primary={project.settings.primaryFormat}
        />
      )}
      <SnapGuides />
    </>
  );
}
