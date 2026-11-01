import { useLingui } from "@lingui/react/macro";
import {
  ChevronLeft,
  ChevronRight,
  Diamond,
  Volume2,
  VolumeX,
} from "lucide-react";
import type { Key } from "../../bindings/Key";
import type { Op } from "../../bindings/Op";
import { IconButton } from "../../design/IconButton";
import clipSettings from "../../lib/clip-settings.json";
import { commitToggle } from "../../state/commit";
import {
  keyIndexAt,
  levelAt,
  nextKey,
  previousKey,
  withKey,
} from "../../lib/keys";
import {
  keySlackUs,
  levelNow,
  levelValue,
  toggleVolumeKey,
  volumeKeyOps,
  type Leveled,
} from "../../state/keyEdits";
import { usePlayback } from "../../state/playback";
import { muteLabel } from "../preview";
import { Field } from "./fields";
import { ToolSlider } from "./ToolSlider";

const percent = (v: number) => `${Math.round(v * 100)}%`;
/** The model's volume range and step (`klipolo_model::clip_settings`), which chat's setClip takes too. */
const VOLUME = clipSettings.volume;

interface VolumeControlsProps {
  volume: number;
  volumeOps: (volume: number) => Op[];
  /**
   * Which layer it mutes (`id`), whether it is muted (as drawn, and `latest`: as the project has it when a press goes
   * in), the ops that leave it muted or not (its own switch, so its volume stays for when it plays again), and what to
   * do once a change has gone in (`landed`). A press is decided and built as it goes in, after a change still on its
   * way (`commitToggle`).
   */
  mute?: {
    id: string;
    muted: boolean;
    latest: () => boolean;
    ops: (muted: boolean) => Op[];
    landed?: (muted: boolean) => void;
  };
  /** What the layer is called in the undo labels: "video" makes "Mute video layer" (`volume.mute.undo`). */
  noun: "clip" | "video" | "sound";
  /**
   * A clip or sound with volume keys (`keys`, from its start at `startUs`): Add volume key at the playhead, and with
   * keys the slider shows and sets the level at the playhead instead of the whole volume.
   */
  line?: {
    target: Leveled;
    keys: Key[] | undefined;
    startUs: number;
    lenUs: number;
  };
}

/**
 * The Volume row: its slider (0 to 200 %, committed on release; sound isn't dry-run) and, when it has one, a Mute
 * toggle beside it.
 */
export function VolumeControls({
  volume,
  volumeOps,
  mute: muting,
  noun,
  line,
}: VolumeControlsProps) {
  const { t } = useLingui();
  const label = t({ id: "volume.label", message: "Volume" });
  const mute = t({ id: "volume.mute", message: "Mute" });
  const muted = muting?.muted ?? false;
  const keyed = !!line?.keys?.length;
  // The playhead in the clip's or sound's time: it follows the playhead only while it has keys or a key can go there.
  const playheadUs = usePlayback((s) => (line ? s.playheadUs : 0));
  const tUs = line ? playheadUs - line.startUs : 0;
  const inside = !!line && tUs >= 0 && tUs <= line.lenUs;
  const here = keyed && keyIndexAt(line!.keys, tUs, keySlackUs()) >= 0;
  const shown = keyed
    ? levelAt(line!.keys, Math.min(Math.max(0, tUs), line!.lenUs), volume)
    : volume;
  const times = line?.keys?.map((k) => k.tUs) ?? [];
  const before = previousKey(times, tUs, keySlackUs());
  const after = nextKey(times, tUs, keySlackUs());
  const seek = (keyUs: number) =>
    usePlayback.getState().seek(line!.startUs + keyUs);
  /** With keys: the key at the playhead holds the level the slider sets (built then, on the keys as they are). */
  const levelOps = (v: number): Op[] => {
    if (!line) return volumeOps(v);
    const now = levelNow(line.target);
    if (now.keys.length === 0) return volumeOps(v);
    const at = Math.min(
      Math.max(0, usePlayback.getState().playheadUs - line.startUs),
      now.lenUs,
    );
    const keys = withKey(now.keys, at, levelValue(v), keySlackUs());
    return keys ? volumeKeyOps(line.target, keys) : [];
  };
  return (
    <Field label={label}>
      <div>
        <div className="volume-row">
          <ToolSlider
            label={label}
            commitLabel="volume.undo"
            value={Math.round(shown * 100) / 100}
            min={VOLUME.min}
            max={VOLUME.max}
            step={VOLUME.step}
            format={percent}
            preview={false}
            ops={levelOps}
          />
          {muting && (
            <IconButton
              className="icon-btn-sm"
              icon={muted ? VolumeX : Volume2}
              label={mute}
              tooltip={
                muted ? t({ id: "volume.unmute", message: "Unmute" }) : mute
              }
              active={muted}
              onClick={() =>
                void commitToggle(
                  muting.latest,
                  (on) => muteLabel(on, noun),
                  muting.ops,
                  { id: muting.id },
                ).then((on) => {
                  if (on !== null) muting.landed?.(on);
                })
              }
            />
          )}
        </div>
        {/* Its keys: the one before the playhead, the key there (added or taken away), the one after. */}
        {line && (
          <div className="volume-keys">
            <IconButton
              className="icon-btn-sm"
              icon={ChevronLeft}
              label={t({
                id: "volume.key.previous",
                message: "Previous volume key",
              })}
              disabled={before === null}
              onClick={() => before !== null && seek(before)}
            />
            <IconButton
              className="icon-btn-sm motion-key"
              icon={
                <Diamond
                  size={14}
                  strokeWidth={2}
                  aria-hidden
                  fill={here ? "currentColor" : "none"}
                />
              }
              label={
                here
                  ? t({ id: "volume.key.remove", message: "Remove volume key" })
                  : t({ id: "volume.key.add", message: "Add volume key" })
              }
              active={here}
              disabled={!inside}
              onClick={() => void toggleVolumeKey(line.target, tUs)}
            />
            <IconButton
              className="icon-btn-sm"
              icon={ChevronRight}
              label={t({ id: "volume.key.next", message: "Next volume key" })}
              disabled={after === null}
              onClick={() => after !== null && seek(after)}
            />
          </div>
        )}
      </div>
    </Field>
  );
}
