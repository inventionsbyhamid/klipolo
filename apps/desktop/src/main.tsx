// First, before React loads: with the perf readout on (`KLIPOLO_PAGE_PERF=1`), its render count hears React's commits.
import './perf/renders';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { suppressContextMenu } from './layout/contextMenu';
import './styles/tailwind.css';
import './styles/tokens.css';
import './styles/app.css';

// Release builds hide the webview's menu (it offers Reload); development keeps it for Inspect Element.
if (import.meta.env.PROD) suppressContextMenu();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
