import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import type { BrowseAsk } from '../../bindings/BrowseAsk';
import type { FontRef } from '../../bindings/FontRef';
import type { FontRow } from '../../bindings/FontRow';
import { resetDownloadsForTests, useFonts } from '../../state/fonts';
import { renderWithProviders } from '../../test/render';
import { FontMenu } from './FontMenu';

/*
 * The font menu: Rust's search and browse (the 12 bundled families first), weights a family has, a dry run on the
 * arrows and a resting hover, and Google Fonts asked once in place before anything downloads.
 */

// `snapped` is the face Rust answers for the asked weight: the fixture gives the family's only or heaviest face.
const row = (family: string, state: FontRow['state'], faces: [number, boolean][] = [[400, false]]): FontRow => ({
  family,
  source: state === 'installed' ? { type: 'installed' } : { type: 'bundled' },
  category: 'sans',
  faces,
  snapped: faces[faces.length - 1],
  state,
  sizeBytes: null,
  moreFaces: [],
  defaultWeightOnly: false,
  systemName: state === 'installed' ? family : null,
  stretch: 5,
  tooLarge: false,
});

const W = [100, 200, 300, 400, 500, 600, 700, 800, 900];
const google = (family: string, faces: [number, boolean][], sizeBytes: number): FontRow => ({
  ...row(family, 'download', faces),
  source: { type: 'google', version: 'v23' },
  snapped: [700, false],
  sizeBytes,
});
const poppins = google(
  'Poppins',
  [...W.map((w): [number, boolean] => [w, false]), ...W.map((w): [number, boolean] => [w, true])],
  2_890_000,
);
const sameFont = (a: FontRef, b: FontRef) =>
  a.family === b.family && JSON.stringify(a.source) === JSON.stringify(b.source);

interface Fake {
  rows: FontRow[];
  google?: { enabled: boolean; consented: boolean };
  download?: (family: string, weight: number, italic: boolean) => Promise<FontRef>;
}

/** Rust's side of the menu: a search over `rows`, a browse of them in sections by window, the chosen font's row. */
function fake(f: Fake) {
  const g = f.google ?? { enabled: true, consented: false };
  const listed = () => f.rows.filter((r) => g.enabled || r.state !== 'download');
  const sections = () =>
    (
      [
        ['bundled', listed().filter((r) => r.state === 'bundled')],
        ['installed', listed().filter((r) => r.state === 'installed')],
        ['google', listed().filter((r) => r.source.type === 'google')],
      ] as [string, FontRow[]][]
    ).filter(([, l]) => l.length > 0);
  const all = () => sections().flatMap(([, l]) => l);
  mockIPC((cmd, args) => {
    const a = args as Record<string, unknown>;
    switch (cmd) {
      case 'fonts_search': {
        const q = (a.query as string).toLowerCase();
        return listed().filter((r) => r.family.toLowerCase().includes(q));
      }
      case 'fonts_browse': {
        const ask = a.ask as BrowseAsk;
        const at = ask.value ? all().findIndex((r) => sameFont(r, ask.value!)) : -1;
        return { sections: sections().map(([s, l]) => [s, l.length]), value: at >= 0 ? at : null };
      }
      case 'fonts_browse_rows':
        return all().slice(a.from as number, a.to as number);
      case 'fonts_row': {
        const font = a.font as FontRef;
        return f.rows.find((r) => sameFont(r, font)) ?? f.rows.find((r) => r.family === font.family) ?? null;
      }
      case 'fonts_google':
        return { ...g };
      case 'fonts_consent':
        g.consented = true;
        return { ...g };
      case 'fonts_download':
        return f.download?.(a.family as string, a.weight as number, a.italic as boolean);
      case 'fonts_face':
      case 'fonts_face_of':
      case 'fonts_preview':
        return new ArrayBuffer(8);
      default:
        return null;
    }
  });
}

const base = () => [
  row('Inter', 'bundled', [
    [400, false],
    [600, false],
    [700, false],
  ]),
  row('Anton', 'bundled'),
  row('Montserrat', 'bundled', [[900, false]]),
  row('Avenir Next', 'installed'),
];

function menu(onPick = vi.fn(), weight = 700, onPreview?: (f: FontRef | null, w: number, i: boolean) => void) {
  renderWithProviders(
    <FontMenu
      label="Font"
      value={{ family: 'Inter', source: { type: 'bundled' } }}
      weight={weight}
      italic={false}
      onPick={onPick}
      onPreview={onPreview}
    />,
  );
  return onPick;
}

const open = () => userEvent.click(screen.getByRole('button', { name: 'Font: Inter' }));

beforeEach(() => {
  resetDownloadsForTests();
  useFonts.setState({ googleFonts: true, googleConsented: false, googleLoaded: false, downloads: {} });
  fake({ rows: base() });
});

describe('the font menu', () => {
  it('opens on the bundled families, then this Mac’s, and searches as you type', async () => {
    menu();
    await open();
    const bundled = await screen.findByText('Included with Klipolo');
    const mac = screen.getByText('On this Mac');
    expect(bundled.compareDocumentPosition(mac) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole('option', { name: /Inter/ })).toHaveAttribute('aria-selected', 'true');
    await userEvent.type(screen.getByRole('combobox', { name: 'Search fonts' }), 'ant');
    expect(await screen.findByRole('option', { name: /Anton/ })).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: /Avenir/ })).not.toBeInTheDocument();
  });

  it('marks fonts only on this Mac and offers only weights the family has', async () => {
    menu();
    await open();
    const avenir = await screen.findByRole('option', { name: /Avenir Next/ });
    expect(within(avenir).getByText('This Mac only')).toBeInTheDocument();
    await userEvent.click(await screen.findByRole('button', { name: 'Weight: Bold' }));
    expect(screen.getAllByRole('menuitemradio').map((e) => e.textContent)).toEqual(['Regular', 'Semibold', 'Bold']);
  });

  it('picking a family snaps to its nearest weight', async () => {
    const onPick = menu();
    await open();
    await userEvent.click(await screen.findByRole('option', { name: /Montserrat/ }));
    expect(onPick).toHaveBeenCalledWith({ family: 'Montserrat', source: { type: 'bundled' } }, 900, false);
  });

  it('previews on the arrows and puts the text back on Escape', async () => {
    const onPreview = vi.fn();
    const onPick = menu(vi.fn(), 400, onPreview);
    await open();
    await screen.findByRole('option', { name: /Anton/ });
    await userEvent.keyboard('{ArrowDown}');
    expect(onPreview).toHaveBeenLastCalledWith(expect.objectContaining({ family: 'Anton' }), 400, false);
    await userEvent.keyboard('{Escape}');
    expect(onPreview).toHaveBeenLastCalledWith(null, 400, false);
    expect(onPick).not.toHaveBeenCalled();
  });

  it('previews the font the pointer rests on, not each one it sweeps past', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    onTestFinished(() => void vi.useRealTimers());
    const onPreview = vi.fn();
    menu(vi.fn(), 400, onPreview);
    fireEvent.click(screen.getByRole('button', { name: 'Font: Inter' }));
    await act(() => vi.advanceTimersByTimeAsync(100));
    const options = screen.getAllByRole('option');
    for (const option of options) fireEvent.pointerEnter(option);
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(onPreview).not.toHaveBeenCalled();
    await act(() => vi.advanceTimersByTimeAsync(100));
    expect(onPreview).toHaveBeenCalledTimes(1);
    expect(onPreview).toHaveBeenLastCalledWith(expect.objectContaining({ family: 'Avenir Next' }), 400, false);
  });

  describe('Google Fonts', () => {
    it('lists them after this Mac’s, with their weights and the download’s size', async () => {
      fake({ rows: [row('Inter', 'bundled'), row('Avenir Next', 'installed'), poppins] });
      menu();
      await open();
      const found = await screen.findByRole('option', { name: /Poppins/ });
      const mac = screen.getByText('On this Mac');
      expect(mac.compareDocumentPosition(screen.getByText('Google Fonts')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(within(found).getByText('9 weights')).toBeInTheDocument();
      expect(within(found).getByText('Download (2.9 MB)')).toBeInTheDocument();
    });

    it('asks once, in place, before anything downloads; then shows its progress and applies the font', async () => {
      let finish: (f: FontRef) => void = () => undefined;
      const download = vi.fn(() => new Promise<FontRef>((done) => (finish = done)));
      fake({ rows: [row('Inter', 'bundled'), poppins], download });
      const onPick = menu();
      await open();
      await userEvent.click(await screen.findByRole('option', { name: /Poppins/ }));
      const consent = screen.getByRole('group', { name: 'Download from Google Fonts?' });
      expect(within(consent).getByRole('button', { name: 'Download (2.9 MB)' })).toHaveFocus();
      expect(download).not.toHaveBeenCalled();
      await userEvent.click(within(consent).getByRole('button', { name: 'Download (2.9 MB)' }));
      await waitFor(() => expect(download).toHaveBeenCalledWith('Poppins', 700, false));
      expect(useFonts.getState().googleConsented).toBe(true);
      expect(await screen.findByRole('progressbar', { name: 'Downloading Poppins' })).toBeInTheDocument();
      const font: FontRef = { family: 'Poppins', source: { type: 'google', version: 'v23' } };
      await act(async () => finish(font));
      expect(onPick).toHaveBeenCalledWith(font, 700, false);
    });

    it('Escape in the consent goes back to the list; once allowed, a pick downloads at once', async () => {
      const download = vi.fn(async () => ({ family: 'Poppins', source: { type: 'google', version: 'v23' } }) as FontRef);
      fake({ rows: [row('Inter', 'bundled'), poppins], download });
      menu();
      await open();
      await userEvent.click(await screen.findByRole('option', { name: /Poppins/ }));
      await userEvent.keyboard('{Escape}');
      expect(screen.queryByRole('group', { name: 'Download from Google Fonts?' })).not.toBeInTheDocument();
      expect(screen.getByRole('combobox', { name: 'Search fonts' })).toHaveFocus();
      expect(download).not.toHaveBeenCalled();
      useFonts.setState({ googleConsented: true });
      await userEvent.click(screen.getByRole('option', { name: /Poppins/ }));
      expect(screen.queryByRole('group', { name: 'Download from Google Fonts?' })).not.toBeInTheDocument();
      await waitFor(() => expect(download).toHaveBeenCalledTimes(1));
    });

    it('lists none that need a download while Google Fonts is off, and says how to turn it on', async () => {
      const lato = { ...google('Lato', [[400, false]], 760_000), state: 'downloaded' as const };
      fake({
        rows: [row('Inter', 'bundled'), row('Avenir Next', 'installed'), poppins, lato],
        google: { enabled: false, consented: true },
      });
      menu(vi.fn(), 400);
      await open();
      await screen.findByRole('option', { name: /Avenir Next/ });
      // What's downloaded stays: it's on this Mac.
      expect(await screen.findByRole('option', { name: /Lato/ })).toBeInTheDocument();
      expect(screen.queryByRole('option', { name: /Poppins/ })).not.toBeInTheDocument();
      expect(screen.getByRole('listbox', { name: 'Fonts' })).toHaveAccessibleDescription(
        'Google Fonts is off. Turn it on in Settings.',
      );
    });
  });
});
