/* ── persistence ────────────────────────────────────────────────────────── */

import type { StateStorage } from "zustand/middleware";

/** localStorage, minus the ways it throws (M23). A write past the origin's
 *  quota throws QuotaExceededError, and a persisted store writes INSIDE `set` —
 *  so the throw came out of whoever called it: an event handler adding a
 *  received text, which then skipped everything after it (its toast, the
 *  history row). Saving is best-effort; the app carrying on is not. */
export const safeLocalStorage: StateStorage = {
  getItem: (name) => {
    try {
      return localStorage.getItem(name);
    } catch {
      return null;
    }
  },
  setItem: (name, value) => {
    try {
      localStorage.setItem(name, value);
    } catch (e) {
      console.warn(`[persist] could not save ${name}`, e);
    }
  },
  removeItem: (name) => {
    try {
      localStorage.removeItem(name);
    } catch {
      // nothing saved, nothing to remove
    }
  },
};

/** How many characters of quick-text bodies one store writes to disk (M23):
 *  about half a megabyte as localStorage counts (UTF-16), so the inbox and the
 *  history together stay well inside the ~5 MB an origin gets. */
export const KEPT_TEXT_CHARS = 256 * 1024;

/** `rows` (newest first) as they are written to disk: each quick text keeps its
 *  full body while the running total fits KEPT_TEXT_CHARS, and the older ones
 *  are saved without it — their preview (`name`) stays. A text can be 60 KiB,
 *  the inbox holds 500 and history 200: saved whole they outgrow localStorage
 *  long before either cap. Only what is SAVED is trimmed; this run keeps every
 *  text it has. */
export function keepRecentTexts<T extends { text?: string }>(rows: T[]): T[] {
  let used = 0;
  return rows.map((r) => {
    if (r.text === undefined) return r;
    used += r.text.length;
    return used <= KEPT_TEXT_CHARS ? r : { ...r, text: undefined };
  });
}
