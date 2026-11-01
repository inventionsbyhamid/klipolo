import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import type { CjkFonts } from '../../bindings/CjkFonts';
import type { CjkScript } from '../../bindings/CjkScript';
import { fonts } from '../../bridge/fonts';

/** Each script's language tag, for its name in the interface's language. */
const LANG: Record<CjkScript, string> = { sc: 'zh-Hans', tc: 'zh-Hant', jp: 'ja', kr: 'ko' };

/** The language a Noto Sans is for, in the interface's language ("Japanese", "Simplified Chinese"). */
export function cjkLanguageName(script: CjkScript, locale: string): string {
  const tag = LANG[script];
  try {
    return new Intl.DisplayNames([locale], { type: 'language' }).of(tag) ?? tag;
  } catch {
    return tag;
  }
}

/**
 * Noto Sans SC, TC, JP and KR as Rust has them: each face's size and whether it is downloaded, asked when shown and
 * again when the fonts change. Null until Rust answers.
 */
export function useCjkFonts(): CjkFonts[] | null {
  const [list, setList] = useState<CjkFonts[] | null>(null);
  useEffect(() => {
    let live = true;
    let stop: (() => void) | null = null;
    const load = () =>
      fonts
        .cjk()
        .then((l) => live && setList(l))
        .catch(() => live && setList([]));
    void load();
    listen('fonts-changed', () => void load())
      .then((unlisten) => (live ? (stop = unlisten) : unlisten()))
      .catch(() => undefined);
    return () => {
      live = false;
      stop?.();
    };
  }, []);
  return list;
}
