/**
 * The design sprite's icons the captions and chat panels need that `layout/Icon` doesn't carry yet. Same grid, stroke
 * and class as `Icon`; they can move into its table when the shared icon set grows.
 */
const PATHS = {
  'eye-off': (
    <path d="M3.5 3.5l17 17M10.4 6.2c.5-.1 1-.2 1.6-.2 5 0 8.5 4.5 9.5 6-.5.8-1.6 2.3-3.2 3.6M6.6 7.7C4.6 9 3.2 11 2.5 12c1 1.5 4.5 6 9.5 6 1.6 0 3-.4 4.3-1.1M9.9 9.9a3 3 0 0 0 4.2 4.2" />
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m20 20-4.2-4.2" />
    </>
  ),
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  /** A favorite: filled while on (`.is-favorite`). */
  star: <path d="M12 4l2.06 5.77 6.12.17-4.85 3.74 1.72 5.88L12 16.1l-5.05 3.46 1.72-5.88-4.85-3.74 6.12-.17z" />,
  range: <path d="M5 5v14M19 5v14M8.5 12h7M8.5 12l2-2M8.5 12l2 2M15.5 12l-2-2M15.5 12l-2 2" />,
  text: <path d="M5 7V5h14v2M12 5v14M9.5 19h5" />,
  retry: (
    <>
      <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3" />
      <path d="M19.5 4.5v4.5H15" />
    </>
  ),
  chat: (
    <path d="M5 4.5h14A1.5 1.5 0 0 1 20.5 6v9.5A1.5 1.5 0 0 1 19 17h-8l-4.5 3.5V17H5a1.5 1.5 0 0 1-1.5-1.5V6A1.5 1.5 0 0 1 5 4.5z" />
  ),
  'expand-panel': (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
      <path d="M14.5 4.5v15M10 10l-2 2 2 2" />
    </>
  ),
} as const;

export type SpriteIconName = keyof typeof PATHS;

export function SpriteIcon({ name, className }: { name: SpriteIconName; className?: string }) {
  return (
    <svg className={className ? `ico ${className}` : 'ico'} viewBox="0 0 24 24" aria-hidden>
      {PATHS[name]}
    </svg>
  );
}
