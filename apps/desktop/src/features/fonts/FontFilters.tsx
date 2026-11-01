import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import type { Category } from '../../bindings/Category';

const KINDS: { value: Category | null; label: MessageDescriptor }[] = [
  { value: null, label: msg({ id: 'fontMenu.kind.all', message: 'All' }) },
  { value: 'sans', label: msg({ id: 'fontMenu.kind.sans', message: 'Sans' }) },
  { value: 'serif', label: msg({ id: 'fontMenu.kind.serif', message: 'Serif' }) },
  { value: 'display', label: msg({ id: 'fontMenu.kind.display', message: 'Display' }) },
  { value: 'handwriting', label: msg({ id: 'fontMenu.kind.handwriting', message: 'Handwriting' }) },
  { value: 'mono', label: msg({ id: 'fontMenu.kind.mono', message: 'Mono' }) },
];

/** The font menu's kind chips and "Covers my text", each a toggle button with `aria-pressed`. */
export function FontFilters({
  category,
  covers,
  onCategory,
  onCovers,
}: {
  category: Category | null;
  covers: boolean;
  onCategory: (c: Category | null) => void;
  onCovers: (on: boolean) => void;
}) {
  const { t, i18n } = useLingui();
  return (
    <div role="group" aria-label={t({ id: 'fontMenu.kinds', message: 'Kind of font' })} className="font-filters">
      {KINDS.map((kind) => (
        <button
          key={kind.value ?? 'all'}
          type="button"
          tabIndex={0}
          className="font-chip"
          aria-pressed={category === kind.value}
          onClick={() => onCategory(kind.value)}
        >
          {i18n._(kind.label)}
        </button>
      ))}
      <button type="button" tabIndex={0} className="font-chip" aria-pressed={covers} onClick={() => onCovers(!covers)}>
        {t({ id: 'fontMenu.covers', message: 'Covers my text' })}
      </button>
    </div>
  );
}
