/**
 * The design sprite's icons the export dialog needs that `layout/Icon` doesn't carry (close, folder). Same grid, stroke
 * and class as `Icon`; they can move into its table when the shared icon set grows.
 */
const PATHS = {
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  folder: <path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />,
} as const;

export type ExportIconName = keyof typeof PATHS;

export function ExportIcon({ name }: { name: ExportIconName }) {
  return (
    <svg className="ico" viewBox="0 0 24 24" aria-hidden>
      {PATHS[name]}
    </svg>
  );
}
