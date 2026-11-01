/** The icon set (24 px grid, 1.8 px stroke, round caps; play and pause are filled), from the design's sprite. */
const PATHS = {
  play: <path className="ico-fill" d="M7 4.9v14.2a.9.9 0 0 0 1.4.8l11.2-7.1a.9.9 0 0 0 0-1.6L8.4 4.1A.9.9 0 0 0 7 4.9z" />,
  pause: (
    <>
      <rect className="ico-fill" x="6" y="5" width="4" height="14" rx="1" />
      <rect className="ico-fill" x="14" y="5" width="4" height="14" rx="1" />
    </>
  ),
  undo: (
    <>
      <path d="M9 14 4 9l5-5" />
      <path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11" />
    </>
  ),
  redo: (
    <>
      <path d="m15 14 5-5-5-5" />
      <path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13" />
    </>
  ),
  settings: (
    <>
      <path d="M4 6.5h9M17 6.5h3M4 12h3M11 12h9M4 17.5h11M19 17.5h1" />
      <circle cx="15" cy="6.5" r="2" />
      <circle cx="9" cy="12" r="2" />
      <circle cx="17" cy="17.5" r="2" />
    </>
  ),
  export: (
    <>
      <path d="M12 15V4M8 8l4-4 4 4" />
      <path d="M5 13v5a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-5" />
    </>
  ),
  scissors: (
    <>
      <circle cx="6" cy="7" r="2.5" />
      <circle cx="6" cy="17" r="2.5" />
      <path d="M8.2 8.4 20 17M8.2 15.6 20 7" />
    </>
  ),
  trash: <path d="M4 7h16M10 7V4.5h4V7M6.5 7l1 12.5h9l1-12.5" />,
  /** A transition: two clips' circles overlapping where they blend. */
  blend: (
    <>
      <circle cx="9" cy="12" r="6" />
      <circle cx="15" cy="12" r="6" />
    </>
  ),
  start: <path d="M6 5v14M18 6.2v11.6L9.5 12z" />,
  back: <path d="M11.5 6.5v11L4 12zM20 6.5v11L12.5 12z" />,
  fwd: <path d="M12.5 6.5v11L20 12zM4 6.5v11L11.5 12z" />,
  loop: (
    <>
      <path d="M17 3.5 20 6.5l-3 3" />
      <path d="M4 11.5v-2a3 3 0 0 1 3-3h13" />
      <path d="M7 20.5l-3-3 3-3" />
      <path d="M20 12.5v2a3 3 0 0 1-3 3H4" />
    </>
  ),
  'zoom-in': (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m20 20-4.2-4.2M8.5 11h5M11 8.5v5" />
    </>
  ),
  'zoom-out': (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m20 20-4.2-4.2M8.5 11h5" />
    </>
  ),
  volume: (
    <>
      <path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" />
      <path d="M15.5 9a4.5 4.5 0 0 1 0 6M18 6.5a8 8 0 0 1 0 11" />
    </>
  ),
  'volume-off': (
    <>
      <path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" />
      <path d="m15.5 9.5 5 5M20.5 9.5l-5 5" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  'chev-down': <path d="m6 9 6 6 6-6" />,
  film: (
    <>
      <rect x="3.5" y="5" width="17" height="14" rx="2" />
      <path d="M7.5 5v14M16.5 5v14M3.5 9.7h4M3.5 14.3h4M16.5 9.7h4M16.5 14.3h4" />
    </>
  ),
  layers: (
    <>
      <path d="m12 4 8.5 4.5L12 13 3.5 8.5z" />
      <path d="m3.5 12.5 8.5 4.5 8.5-4.5" />
      <path d="m3.5 16 8.5 4.5 8.5-4.5" />
    </>
  ),
  pulse: <path d="M3 12h3.5l2-5.5 3.5 11 2.5-8 1.5 2.5H21" />,
  music: (
    <>
      <path d="M9 17.5V6.5l11-2v11" />
      <circle cx="6.5" cy="17.5" r="2.5" />
      <circle cx="17.5" cy="15.5" r="2.5" />
    </>
  ),
  'new-chat': (
    <>
      <path d="M12 4.5H5A1.5 1.5 0 0 0 3.5 6v9.5A1.5 1.5 0 0 0 5 17h1.5v3.5L11 17h8a1.5 1.5 0 0 0 1.5-1.5V12" />
      <path d="M18 2.5v6M15 5.5h6" />
    </>
  ),
  collapse: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
      <path d="M14.5 4.5v15M7.5 10l2 2-2 2" />
    </>
  ),
  send: <path d="M12 19V5.5M6.5 11 12 5.5l5.5 5.5" />,
  safe: (
    <>
      <path d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16" />
      <rect x="8" y="8" width="8" height="8" rx="1" />
    </>
  ),
  full: <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />,
  'full-exit': <path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5" />,
  check: <path d="m5 12.5 4.5 4.5L19 7.5" />,
  info: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 11v5.5M12 7.7v.1" />
    </>
  ),
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  image: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
      <circle cx="9" cy="10" r="1.8" />
      <path d="m4 18 5-5 4 4 2.5-2.5L20 19" />
    </>
  ),
  text: <path d="M5 7V5h14v2M12 5v14M9.5 19h5" />,
  camera: (
    <>
      <rect x="3.5" y="6.5" width="12" height="11" rx="2" />
      <path d="m15.5 10.5 5-3v9l-5-3" />
    </>
  ),
  more: (
    <>
      <circle className="ico-fill" cx="6" cy="12" r="1.5" />
      <circle className="ico-fill" cx="12" cy="12" r="1.5" />
      <circle className="ico-fill" cx="18" cy="12" r="1.5" />
    </>
  ),
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg className={className ? `ico ${className}` : 'ico'} viewBox="0 0 24 24" aria-hidden>
      {PATHS[name]}
    </svg>
  );
}
