import { type ReactNode, useId } from 'react';
import type { DeviceInfo } from '../../bindings/DeviceInfo';
import { Menu, MenuContent, MenuRadioGroup, MenuRadioItem, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';

/** The value "No microphone" or "No camera" stands for in the menu (device ids are never empty). */
const NONE = '';

const WORDS = {
  mic: { label: 'Microphone', none: 'No microphone' },
  camera: { label: 'Camera', none: 'No camera' },
} as const;

interface DeviceSelectProps {
  kind: 'mic' | 'camera';
  devices: DeviceInfo[];
  /** The device chosen, by id; null is "No microphone" or "No camera". */
  value: string | null;
  onPick: (id: string | null) => void;
  /** What shows under the select: a refused permission, the toggles, the meter. */
  children?: ReactNode;
}

/**
 * The microphone or the camera a take records, or none (a select: a 30 px button with a chevron that opens a
 * menu). Devices are listed by their own names, without opening them. The mics come first and "No microphone" last;
 * "No camera" comes first, as a camera is the exception.
 */
export function DeviceSelect({ kind, devices, value, onPick, children }: DeviceSelectProps) {
  const { label, none } = WORDS[kind];
  const labelId = useId();
  const listed = devices.map((d) => ({ value: d.id, label: d.name }));
  const choices = kind === 'mic' ? [...listed, { value: NONE, label: none }] : [{ value: NONE, label: none }, ...listed];
  const shown = devices.find((d) => d.id === value)?.name ?? none;
  return (
    <div className="record-field">
      <span className="record-label" id={labelId}>
        {label}
      </span>
      <Menu>
        <MenuTrigger asChild>
          <button type="button" className="select" aria-label={`${label}: ${shown}`}>
            <span className="select-text">{shown}</span>
            <Icon name="chev-down" />
          </button>
        </MenuTrigger>
        <MenuContent>
          <MenuRadioGroup
            label={label}
            value={value ?? NONE}
            onValueChange={(next) => onPick(next === NONE ? null : next)}
          >
            {choices.map((c) => (
              <MenuRadioItem key={c.value} value={c.value}>
                {c.label}
              </MenuRadioItem>
            ))}
          </MenuRadioGroup>
        </MenuContent>
      </Menu>
      {children}
    </div>
  );
}
