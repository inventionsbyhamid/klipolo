import { useMemo } from 'react';
import type { FontRef } from '../../bindings/FontRef';
import { sameFont } from '../../state/fonts';
import { useProject } from '../../state/project';

/** The fonts the open project's texts use (`In this project`), each once. */
export function useProjectFonts(): FontRef[] {
  const tracks = useProject((s) => s.snapshot?.project.timeline.tracks);
  return useMemo(() => {
    const out: FontRef[] = [];
    for (const item of (tracks ?? []).flatMap((t) => t.items)) {
      const font = item.content.type === 'text' ? item.content.style.font : null;
      if (font && !out.some((f) => sameFont(f, font))) out.push(font);
    }
    return out;
  }, [tracks]);
}
