import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { applyTheme, useThemeStore } from './lib/theme';
import { applyZoom, useZoomStore } from './lib/zoom';

// Read from the stores here rather than left to a render: the window has to be at
// its stored size on the first paint, or it is drawn at the old one and then jumps.
applyTheme(useThemeStore.getState().mode);
applyZoom(useZoomStore.getState().level);

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
