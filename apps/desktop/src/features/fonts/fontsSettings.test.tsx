import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import type { DownloadedFamily } from '../../bindings/DownloadedFamily';
import { useFonts } from '../../state/fonts';
import { renderWithProviders } from '../../test/render';
import { FontsSettings } from './FontsSettings';

/** Rust's side: the setting, the downloaded families, what was asked. */
function fake(downloaded: DownloadedFamily[]) {
  const g = { enabled: true, consented: true };
  const asked: [string, unknown][] = [];
  mockIPC((cmd, args) => {
    asked.push([cmd, args]);
    switch (cmd) {
      case 'fonts_google':
        return { ...g };
      case 'fonts_set_google':
        g.enabled = (args as { enabled: boolean }).enabled;
        return { ...g };
      case 'fonts_downloaded':
        return downloaded;
      case 'fonts_remove': {
        const { id } = args as { id: { family: string } };
        downloaded.splice(
          downloaded.findIndex((f) => f.id.family === id.family),
          1,
        );
        return null;
      }
      case 'fonts_catalogue':
        return { date: '2026-10-22', sample: false, families: 1900 };
      default:
        return null;
    }
  });
  return asked;
}

const lato: DownloadedFamily = { id: { family: 'Lato', version: 'v24' }, bytes: 760_000, faces: [[400, false]] };

beforeEach(() => useFonts.setState({ googleFonts: true, googleConsented: true, googleLoaded: false, downloads: {} }));

describe('Settings, Fonts', () => {
  it('turns Google Fonts off and on, and says which font list Klipolo ships', async () => {
    const asked = fake([]);
    renderWithProviders(<FontsSettings />);
    const google = screen.getByRole('switch', { name: 'Google Fonts' });
    expect(google).toHaveAccessibleDescription('Search and download free fonts from Google.');
    expect(await screen.findByText('Font list from October 22, 2026')).toBeInTheDocument();
    expect(await screen.findByText('No Google fonts downloaded yet.')).toBeInTheDocument();
    await userEvent.click(google);
    await waitFor(() => expect(useFonts.getState().googleFonts).toBe(false));
    expect(asked).toContainEqual(['fonts_set_google', { enabled: false }]);
  });

  it('lists the downloaded fonts with their sizes, and Remove asks in place before it removes one', async () => {
    const asked = fake([lato]);
    renderWithProviders(<FontsSettings />);
    expect(await screen.findByText('Lato')).toBeInTheDocument();
    expect(screen.getByText(/^760\s?kB$/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Remove Lato' }));
    const ask = screen.getByRole('group', { name: 'Remove Lato?' });
    expect(within(ask).getByText('Projects that use Lato will show it as missing.')).toBeInTheDocument();
    expect(within(ask).getByRole('button', { name: 'Cancel' })).toHaveFocus();
    expect(asked.some(([cmd]) => cmd === 'fonts_remove')).toBe(false);
    await userEvent.click(within(ask).getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(asked).toContainEqual(['fonts_remove', { id: lato.id }]));
    expect(await screen.findByText('No Google fonts downloaded yet.')).toBeInTheDocument();
  });
});
