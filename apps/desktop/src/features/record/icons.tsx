/**
 * The Record dialog's glyphs that `layout/Icon` doesn't carry yet: a screen, an app's window, the record ring and the
 * alert. Same grid, stroke and class as `Icon`; they can move into its table when the shared icon set grows.
 */
const PATHS = {
  screen: (
    <>
      <rect x="3.5" y="4.5" width="17" height="11.5" rx="2" />
      <path d="M9 19.5h6M12 16v3.5" />
    </>
  ),
  app: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
      <path d="M3.5 9h17M6.5 6.8h.1M9 6.8h.1" />
    </>
  ),
  // A ring with a filled dot: in the button's ink, never red.
  record: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <circle className="ico-fill" cx="12" cy="12" r="4" />
    </>
  ),
  alert: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7.5V13M12 16.3v.1" />
    </>
  ),
} as const;

export type RecordIconName = keyof typeof PATHS;

export function RecordIcon({ name }: { name: RecordIconName }) {
  return (
    <svg className="ico" viewBox="0 0 24 24" aria-hidden>
      {PATHS[name]}
    </svg>
  );
}
