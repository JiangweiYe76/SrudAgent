// Window zoom rather than a text size setting, because the app's font sizes are a
// mix: Tailwind's rem-based scale on one side, pixels picked a step at a time in
// the components and in `styles.css` on the other. A root `font-size` moves the
// first and leaves the second alone, so the window would come out half resized.
// Scaling the window cannot come out half anything.
//
// The scale is the webview's, not CSS's. `zoom` on an element scales that
// element's whole subtree a second time, so anything positioned against the
// viewport comes out doubled: a dropdown in a portal measures its trigger in the
// scaled space, works out a `translate`, and has the result scaled again on the
// way out. The webview scales the rendered page instead, which leaves one
// coordinate system for both the measurement and the positioning.
import { create } from 'zustand';
import { getCurrentWebview } from '@tauri-apps/api/webview';

export const ZOOM_LEVELS = [0.8, 1, 1.25, 1.5, 2] as const;

export type ZoomLevel = (typeof ZOOM_LEVELS)[number];

const STORAGE_KEY = 'srud.zoom';

function readStoredLevel(): ZoomLevel {
  const raw = Number(localStorage.getItem(STORAGE_KEY));
  // Matched against the offered list rather than range-checked: a level between
  // two offered ones has no button to show as chosen, which would put the
  // settings in a state they cannot represent.
  return ZOOM_LEVELS.find((level) => level === raw) ?? 1;
}

interface ZoomState {
  level: ZoomLevel;
  setLevel: (level: ZoomLevel) => Promise<void>;
}

export const useZoomStore = create<ZoomState>((set) => ({
  level: readStoredLevel(),
  // Told before it is recorded, so the settings cannot show a level the window has
  // not caught up to yet. The window is the slow one, and a settings row that says
  // 150% while the window is still at 100% reads as the click having done nothing.
  setLevel: async (level) => {
    await applyZoom(level);
    localStorage.setItem(STORAGE_KEY, String(level));
    set({ level });
  },
}));

export function percentOf(level: ZoomLevel): string {
  return `${Math.round(level * 100)}%`;
}

// Swallows a refused zoom rather than throwing it at the caller, because both
// callers are places where a throw is worse than a window at the wrong size: one
// is the first paint, where an unhandled rejection means no app at all, and the
// other is a settings row, where the level is still worth keeping. Reported
// rather than dropped, so a webview without zoom support is visible instead of
// looking like a click that did nothing.
export async function applyZoom(level: ZoomLevel): Promise<void> {
  try {
    await getCurrentWebview().setZoom(level);
  } catch (err) {
    console.warn('zoom not applied', level, err);
  }
}