// Window zoom rather than a text size setting, because the app's font sizes are a
// mix: Tailwind's rem-based scale on one side, pixels picked a step at a time in
// the components and in `styles.css` on the other. A root `font-size` moves the
// first and leaves the second alone, so the window would come out half resized.
// Scaling the window cannot come out half anything.
import { create } from 'zustand';

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
  setLevel: (level: ZoomLevel) => void;
}

export const useZoomStore = create<ZoomState>((set) => ({
  level: readStoredLevel(),
  // Applied from here rather than from a component's effect, so that recording a
  // level and having it take effect cannot come apart.
  setLevel: (level) => {
    localStorage.setItem(STORAGE_KEY, String(level));
    applyZoom(level);
    set({ level });
  },
}));

export function percentOf(level: ZoomLevel): string {
  return `${Math.round(level * 100)}%`;
}

export function applyZoom(level: ZoomLevel): void {
  document.documentElement.style.zoom = String(level);
}