/**
 * The design sprite's icons the AI surfaces need that `layout/Icon` doesn't carry yet (key, ticket, close). Same grid,
 * stroke and class as `Icon`; they can move into its table when the shared icon set grows.
 */
const PATHS = {
  key: (
    <>
      <circle cx="8" cy="15" r="4" />
      <path d="m11 12 8.5-8.5M16.5 6.5 19 9M14 9l2 2" />
    </>
  ),
  ticket: (
    <>
      <path d="M4 7.5A1.5 1.5 0 0 1 5.5 6h13A1.5 1.5 0 0 1 20 7.5V10a2 2 0 0 0 0 4v2.5a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 16.5V14a2 2 0 0 0 0-4z" />
      <path d="M14.5 8.5v1.2M14.5 11.4v1.2M14.5 14.3v1.2" />
    </>
  ),
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
} as const;

export type AiIconName = keyof typeof PATHS;

export function AiIcon({ name }: { name: AiIconName }) {
  return (
    <svg className="ico" viewBox="0 0 24 24" aria-hidden>
      {PATHS[name]}
    </svg>
  );
}
