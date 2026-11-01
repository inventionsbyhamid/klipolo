import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { FontRef } from '../../bindings/FontRef';
import type { FontRow } from '../../bindings/FontRow';
import type { MissingFont } from '../../bindings/MissingFont';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { resetDownloadsForTests, useFonts } from '../../state/fonts';
import { resetMissingForTests } from '../../state/missingFonts';
import { useProject } from '../../state/project';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { MissingFontsBanner } from './MissingFontsBanner';

/*
 * The missing-fonts banner: how many fonts the project uses that aren't on this Mac, Download when every one can be,
 * Use {family} {version} for a version Google no longer serves, Replace fonts… with a font menu per face, and Dismiss.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() => Promise.resolve(true));
const poppins = (version: string): FontRef => ({ family: 'Poppins', source: { type: 'google', version } });

/** The fixture with its title in `font`: a copy of the title, which the fixtures share between tests. */
function openWithTitleIn(font: FontRef) {
  const snapshot = fixtureSnapshot();
  for (const track of snapshot.project.timeline.tracks)
    track.items = track.items.map((i) =>
      i.id === 'title' && i.content.type === 'text'
        ? { ...i, content: { ...i.content, style: { ...i.content.style, font, weight: 700 } } }
        : i,
    );
  useProject.setState({ snapshot, pending: 0, apply });
}

const missing = (m: Partial<MissingFont>): MissingFont => ({
  font: poppins('v22'),
  weights: [[700, false]],
  layers: ['title'],
  hiddenLayers: [],
  sceneParams: [],
  inScenes: false,
  captions: false,
  downloadBytes: null,
  current: null,
  cjk: null,
  ...m,
});

const montserrat: FontRow = {
  family: 'Montserrat',
  source: { type: 'bundled' },
  category: 'sans',
  faces: [[900, false]],
  snapped: [900, false],
  state: 'bundled',
  sizeBytes: null,
  moreFaces: [],
  defaultWeightOnly: false,
  systemName: null,
  stretch: 5,
  tooLarge: false,
};

/** Rust: the project's missing fonts, a browse of Montserrat for the font menus, and the downloads asked. */
function fake(fonts: MissingFont[]) {
  const downloads: unknown[] = [];
  mockIPC((cmd, args) => {
    switch (cmd) {
      case 'fonts_missing':
        return { type: 'ready', fonts };
      case 'fonts_browse':
        return { sections: [['bundled', 1]], value: null };
      case 'fonts_browse_rows':
        return [montserrat];
      case 'fonts_google':
        return { enabled: true, consented: true };
      case 'fonts_download':
        downloads.push(args);
        return poppins('v22');
      default:
        return null;
    }
  });
  return downloads;
}

beforeEach(() => {
  vi.clearAllMocks();
  resetMissingForTests();
  resetDownloadsForTests();
  useFonts.setState({ googleFonts: true, googleConsented: true, googleLoaded: true, downloads: {} });
});

describe('the missing-fonts banner', () => {
  it('names how many, downloads them at the project’s version, and replaces them in one step', async () => {
    openWithTitleIn(poppins('v22'));
    const downloads = fake([missing({ downloadBytes: 2_890_000 })]);
    renderWithProviders(<MissingFontsBanner />);
    expect(await screen.findByText('1 font used here isn’t on this Mac.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Download (2.9 MB)' }));
    await waitFor(() => expect(downloads).toEqual([{ family: 'Poppins', version: 'v22', weight: 700, italic: false }]));

    await userEvent.click(screen.getByRole('button', { name: 'Replace fonts…' }));
    const dialog = screen.getByRole('dialog', { name: 'Replace fonts' });
    expect(within(dialog).getByText('Poppins Bold')).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Font: Poppins' }));
    await userEvent.click(await screen.findByRole('option', { name: /Montserrat/ }));
    await userEvent.click(within(screen.getByRole('dialog', { name: 'Replace fonts' })).getByRole('button', { name: 'Replace' }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply.mock.calls[0][0]).toBe('fontMenu.replace.undo');
    const ops = apply.mock.calls[0][1] as unknown as { id: string; patch: { content: { style: unknown } } }[];
    expect(ops).toHaveLength(1);
    expect(ops[0].id).toBe('title');
    // The title takes Montserrat at the face Rust snapped it to.
    expect(ops[0].patch.content.style).toMatchObject({ font: { family: 'Montserrat' }, weight: 900 });
  });

  it('offers the current version when Google no longer serves the project’s one', async () => {
    openWithTitleIn(poppins('v20'));
    fake([missing({ font: poppins('v20'), current: poppins('v22') })]);
    renderWithProviders(<MissingFontsBanner />);
    expect(await screen.findByText('1 font used here isn’t on this Mac.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Download/ })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Use Poppins v22' }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply.mock.calls[0][0]).toBe('fontMenu.replace.undo');
    expect(JSON.stringify(apply.mock.calls[0][1])).toContain('"version":"v22"');
  });

  it('says what Chinese, Japanese or Korean text needs, and Dismiss hides it for this opening', async () => {
    openWithTitleIn(poppins('v22'));
    fake([missing({}), missing({ font: { family: 'Noto Sans JP', source: { type: 'google', version: 'v52' } }, cjk: 'jp' })]);
    renderWithProviders(<MissingFontsBanner />);
    expect(await screen.findByText('2 fonts used here aren’t on this Mac.')).toBeInTheDocument();
    expect(screen.getByText('Japanese text needs Noto Sans JP.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByText(/aren’t on this Mac/)).toBeNull();
  });
});
