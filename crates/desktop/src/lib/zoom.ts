import { create } from 'zustand';
import { getCurrentWebview } from '@tauri-apps/api/webview';

// Window zoom rather than a text size setting, because the app's font sizes are a
// mix: Tailwind's rem-based scale on one side, pixels picked a step at a time in
// the components and in `styles.css` on the other. A root `font-size` moves the
// first and leaves the second alone, so the window would come out half resized.
// Scaling the window cannot come out half anything.
//
// The scale is the webview's, not CSS's. A CSS `zoom` multiplies the offset of
// everything positioned inside it, so a trigger's rect gets measured in the scaled
// space, the `translate` worked out from it gets scaled again on the way out, and a
// dropdown lands further from its trigger the higher the zoom goes — measured 80 px
// adrift at 125% and 520 px at 200%. The webview zooms the rendered page instead,
// which leaves one coordinate space for the measurement and the positioning.

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

// Counts the calls to `setLevel` so that a slow one that answers after a newer one
// can tell it is stale and leave the store alone. Two clicks in quick succession put
// two calls in flight, and the webview does not promise to answer them in order —
// without this the older one can answer last and overwrite the newer level, leaving
// the button claiming 150% while the window sits at 200%.
let requested = 0;

export const useZoomStore = create<ZoomState>((set) => ({
  level: readStoredLevel(),
  // The webview is told before the level is recorded, so a button cannot show as
  // chosen a level the window is not at yet — one showing 150% while the window is
  // still at 100% reads as the click having done nothing.
  setLevel: async (level) => {
    const mine = ++requested;
    await applyZoom(level);
    if (mine !== requested) return;
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
// other is a settings button, where the level is still worth keeping. Reported
// rather than dropped, so a webview that cannot zoom shows up here instead of
// looking like a click that did nothing.
export async function applyZoom(level: ZoomLevel): Promise<void> {
  try {
    await getCurrentWebview().setZoom(level);
  } catch (err) {
    console.warn('zoom not applied', level, err);
  }
}