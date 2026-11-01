import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import type { MoreFace } from '../../bindings/MoreFace';
import {
  Menu,
  MenuContent,
  MenuItem,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuTrigger,
} from '../../design/Menu';
import { useIntlLocale } from '../../i18n/react';
import { formatDownloadSize } from '../../lib/format';
import { Icon } from '../../layout/Icon';

/** CSS weights' common names. */
const NAMES: Record<number, MessageDescriptor> = {
  100: msg({ id: 'fontMenu.weight.100', message: 'Thin' }),
  200: msg({ id: 'fontMenu.weight.200', message: 'Extra light' }),
  300: msg({ id: 'fontMenu.weight.300', message: 'Light' }),
  400: msg({ id: 'fontMenu.weight.400', message: 'Regular' }),
  500: msg({ id: 'fontMenu.weight.500', message: 'Medium' }),
  600: msg({ id: 'fontMenu.weight.600', message: 'Semibold' }),
  700: msg({ id: 'fontMenu.weight.700', message: 'Bold' }),
  800: msg({ id: 'fontMenu.weight.800', message: 'Extra bold' }),
  900: msg({ id: 'fontMenu.weight.900', message: 'Black' }),
};

/** A face's name: its weight's ("Bold"), the nearest named one for an in-between weight, and "italic" after it. */
export function useWeightName() {
  const { t, i18n } = useLingui();
  return (weight: number, italic: boolean) => {
    const named = Math.min(900, Math.max(100, Math.round(weight / 100) * 100));
    const name = i18n._(NAMES[named]);
    return italic
      ? t({
          id: 'fontMenu.weight.italic',
          message: `${name} italic`,
          comment: '{name} is a weight’s name, such as “Bold”',
        })
      : name;
  };
}

/**
 * The chosen family's weights, only those it has (a pick never asks for a face that isn't there). Its button is
 * named "Weight: {name}". A Google family downloaded a weight at a time also offers the weights it can gain, each as
 * "Download {weight} ({size})": picked, that face downloads, then the text takes it.
 */
export function WeightMenu({
  faces,
  weight,
  italic,
  onPick,
  more = [],
  onDownload,
}: {
  faces: [number, boolean][];
  weight: number;
  italic: boolean;
  onPick: (weight: number, italic: boolean) => void;
  more?: MoreFace[];
  onDownload?: (face: MoreFace) => void;
}) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const nameOf = useWeightName();
  const current = nameOf(weight, italic);
  const sorted = [...faces].sort((a, b) => a[0] - b[0] || Number(a[1]) - Number(b[1]));
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          className="select font-weight"
          aria-label={t({
            id: 'fontMenu.weight',
            message: `Weight: ${current}`,
            comment: '{current} is a weight’s name, such as “Bold”',
          })}
        >
          <span className="select-text">{current}</span>
          <Icon name="chev-down" />
        </button>
      </MenuTrigger>
      <MenuContent>
        <MenuRadioGroup
          value={`${weight}|${italic ? 'i' : 'n'}`}
          onValueChange={(value) => {
            const [w, style] = value.split('|');
            onPick(Number(w), style === 'i');
          }}
        >
          {sorted.map(([w, i]) => (
            <MenuRadioItem key={`${w}|${i ? 'i' : 'n'}`} value={`${w}|${i ? 'i' : 'n'}`}>
              {nameOf(w, i)}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
        {onDownload && more.length > 0 && (
          <>
            <MenuSeparator />
            {more.map((face) => {
              const name = nameOf(face.weight, face.italic);
              const size = formatDownloadSize(locale, face.bytes);
              return (
                <MenuItem key={`${face.weight}|${face.italic ? 'i' : 'n'}`} onSelect={() => onDownload(face)}>
                  {t({
                    id: 'fontMenu.weight.download',
                    message: `Download ${name} (${size})`,
                    comment:
                      '{name} is a weight’s name (“Bold”) of a Google font downloaded a weight at a time, {size} its download (“5.4 MB”)',
                  })}
                </MenuItem>
              );
            })}
          </>
        )}
      </MenuContent>
    </Menu>
  );
}
