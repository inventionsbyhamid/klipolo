import { render } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import { TooltipProvider } from '../design/Tooltip';
import { LocaleProvider } from '../i18n/react';

/** The providers every screen renders inside (minus the toaster and error screen). */
export function TestProviders({ children }: { children: ReactNode }) {
  return (
    <LocaleProvider>
      <TooltipProvider>{children}</TooltipProvider>
    </LocaleProvider>
  );
}

export function renderWithProviders(ui: ReactElement) {
  return render(ui, { wrapper: TestProviders });
}
