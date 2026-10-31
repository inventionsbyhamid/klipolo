import type { ReactElement, ReactNode } from 'react';
import { useProject } from '../state/project';
import { renderWithProviders } from './render';

/** What the app shows only while a project is open (its editor): gone once the close lands, as the editor goes. */
function WhileOpen({ children }: { children: ReactNode }) {
  const open = useProject((s) => s.snapshot !== null);
  return open ? children : null;
}

/** `ui` rendered as the editor holds it: only while a project is open. */
export const renderOpen = (ui: ReactElement) => renderWithProviders(<WhileOpen>{ui}</WhileOpen>);
