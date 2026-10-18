// What each session's input box holds, for this machine only.

const STORAGE_KEY = 'srud.drafts';

export function loadDrafts(): Record<string, string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) return {};
    const drafts: Record<string, string> = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (typeof value === 'string') drafts[key] = value;
    }
    return drafts;
  } catch {
    return {};
  }
}

export function saveDrafts(drafts: Record<string, string>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(drafts));
  } catch {
    // A webview that refuses the write keeps the drafts for this run only.
  }
}
