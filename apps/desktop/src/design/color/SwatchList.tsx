import { Ban } from 'lucide-react';
import { Fragment, useLayoutEffect, useRef, useState, type FocusEvent, type JSX, type KeyboardEvent } from 'react';
import { cn } from '../cn';
import { Tooltip } from '../Tooltip';
import { sameColor, sameRgb } from './colorMath';

export interface SwatchOption {
  /** `#RRGGBB(AA)`; null for the none choice. */
  value: string | null;
  /** What it is announced and tooltipped as: "Yellow", "No outline". */
  name: string;
  /** A colour (the default) shows its fill; none a ⊘. */
  kind?: 'color' | 'none';
}

export interface SwatchGroup {
  id: string;
  /** A labelled group is a `role="group"` under a small heading; an unlabelled one sits inline with the rest. */
  label?: string;
  options: SwatchOption[];
}

interface SwatchListProps {
  label: string;
  groups: SwatchGroup[];
  /**
   * The colour the setting has: its swatch is selected, and the list's one tab stop while focus is elsewhere.
   * Undefined: none is selected.
   */
  value: string | null | undefined;
  /** Hovered or focused: dry-run it. */
  onPreview(v: string | null): void;
  /** The pointer or focus left the swatches, or Escape: put the video back. */
  onPreviewEnd(): void;
  /** Clicked, or Enter or Space: one change. */
  onPick(v: string | null): void;
  /** The setting takes an alpha: the swatch of its red, green and blue is selected, whatever the opacity. */
  alpha?: boolean;
}

/** A swatch's identity in its list (each is there once): what the roving tab stop follows as swatches come and go. */
const idOf = (option: SwatchOption) => `${option.kind ?? 'color'}:${option.value ?? ''}`;

/** Focuses `el` quietly: no dry run (`onFocus` skips it) and, as for any swatch focus, no tooltip. */
function focusQuietly(el: HTMLElement) {
  el.dataset.quiet = '';
  el.focus();
  delete el.dataset.quiet;
}

/** The arrows move to the previous or next swatch; ↑ and ↓ do as ← and →, since the rows wrap. */
const STEPS: Record<string, number> = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 };

/**
 * Focuses the selected swatch in `container` (else the first) quietly, as a popover opens: no dry run (the first
 * swatch of a custom colour would show on the video just by opening).
 */
export function focusSwatch(container: Element) {
  const options = Array.from(container.querySelectorAll<HTMLElement>('[role="option"]'));
  const target = options.find((o) => o.getAttribute('aria-selected') === 'true') ?? options[0];
  if (target) focusQuietly(target);
}

/**
 * Colour swatches as a listbox, in labelled groups (the color popover's): one tab stop, a roving one (the focused
 * swatch while focus is in the list, so Tab and Shift+Tab leave it from any swatch; the selected one, else the first,
 * once focus has left, so Tab back in lands there); the arrows, Home and End move between them, Enter, Space or a
 * click picks one. Each is a 20 px circle named by its colour, with a tooltip on hover only: one open on keyboard
 * focus would take the first Escape, which should close the popover. Hovering or focusing one previews it; leaving the
 * list, or Escape, ends the preview (Escape still reaches the popover around it, which closes).
 */
export function SwatchList({
  label,
  groups,
  value,
  onPreview,
  onPreviewEnd,
  onPick,
  alpha = false,
}: SwatchListProps): JSX.Element {
  const refs = useRef<(HTMLDivElement | null)[]>([]);
  /** The swatch focus is on (`idOf`), while it is in the list (the roving tab stop); null once it has left. */
  const [focused, setFocused] = useState<string | null>(null);
  const all = groups.flatMap((g) => g.options);
  const same = (a: string | null, b: string | null) =>
    alpha && a !== null && b !== null ? sameRgb(a, b) : sameColor(a, b);
  const selected = value === undefined ? -1 : all.findIndex((o) => same(o.value, value));
  const focusedAt = focused === null ? -1 : all.findIndex((o) => idOf(o) === focused);
  const tabStop = focusedAt >= 0 ? focusedAt : Math.max(0, selected);
  // The list changed under focus and took the focused swatch with it: focus fell to the page, where the editor's keys
  // would take the arrows, Space and Delete. It goes back to that colour (else the tab stop) quietly, in the same
  // commit, so it never visibly leaves.
  useLayoutEffect(() => {
    if (focused === null) return;
    const active = document.activeElement;
    if (active !== null && active !== document.body) return;
    const target = refs.current[tabStop];
    if (target?.isConnected) focusQuietly(target);
  });

  const onKeyDown = (index: number, option: SwatchOption) => (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      onPreviewEnd();
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const step = STEPS[e.key];
    const to = step !== undefined ? index + step : e.key === 'Home' ? 0 : e.key === 'End' ? all.length - 1 : null;
    if (to !== null) {
      e.preventDefault();
      refs.current[Math.min(all.length - 1, Math.max(0, to))]?.focus();
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onPick(option.value);
    }
  };
  const onBlur = (e: FocusEvent<HTMLDivElement>) => {
    if (e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
    setFocused(null);
    onPreviewEnd();
  };

  let index = 0;
  const swatch = (option: SwatchOption) => {
    const at = index++;
    const isSelected = at === selected;
    const kind = option.kind ?? 'color';
    return (
      <Tooltip key={`${kind}:${option.value ?? ''}:${option.name}`} label={option.name}>
        <div
          ref={(el) => {
            refs.current[at] = el;
          }}
          role="option"
          aria-label={option.name}
          aria-selected={isSelected}
          tabIndex={at === tabStop ? 0 : -1}
          onFocus={(e) => {
            // No tooltip on keyboard focus: a prevented focus skips Radix's open (hover still opens it).
            e.preventDefault();
            setFocused(idOf(option));
            // `focusSwatch` (a popover opening): nothing is dry-run.
            if (e.currentTarget.dataset.quiet === undefined) onPreview(option.value);
          }}
          onPointerEnter={() => onPreview(option.value)}
          onClick={() => onPick(option.value)}
          onKeyDown={onKeyDown(at, option)}
          className={cn('swatch', kind === 'none' && 'none')}
          // The fill is a content colour the person can pick, so it's data, not a token.
          style={kind === 'color' && option.value ? { ['--c' as string]: option.value } : undefined}
        >
          {kind === 'none' && <Ban size={12} aria-hidden />}
        </div>
      </Tooltip>
    );
  };

  return (
    <div
      role="listbox"
      aria-label={label}
      aria-orientation="horizontal"
      onPointerLeave={onPreviewEnd}
      onBlur={onBlur}
      className="swatch-list"
    >
      {groups.map((group) =>
        group.label ? (
          <div key={group.id} role="group" aria-label={group.label} className="swatch-group">
            <span aria-hidden className="swatch-group-label">
              {group.label}
            </span>
            <div className="swatches">{group.options.map(swatch)}</div>
          </div>
        ) : (
          <Fragment key={group.id}>{group.options.map(swatch)}</Fragment>
        ),
      )}
    </div>
  );
}
