/* ── recents (files offered in the send flow) ───────────────────────────── */

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import { baseName, extOf } from "../format";
import { safeLocalStorage } from "./persist";

export type SendFile = {
  path?: string;
  name: string;
  ext: string;
  size?: number;
  /** A preview (`data:` URL) — a pasted clipboard image has one, since its
   *  file name says nothing about what it shows. */
  thumb?: string;
  /** Written by the app for this send (a pasted image) and aged out of its
   *  cache: not something to offer again later as a recent file. */
  temp?: boolean;
};

interface RecentsState {
  items: SendFile[];
  add: (files: SendFile[]) => void;
}

export const useRecents = create<RecentsState>()(
  persist(
    (set) => ({
      items: [],
      add: (files) =>
        set((s) => {
          // A temp file's path stops existing; a recent that cannot be sent
          // is worse than no recent.
          const kept = files.filter((f) => !f.temp);
          if (!kept.length) return s;
          const seen = new Set(kept.map((f) => f.path ?? f.name));
          const rest = s.items.filter((f) => !seen.has(f.path ?? f.name));
          return { items: [...kept, ...rest].slice(0, 8) };
        }),
    }),
    {
      name: "lanbeam.recents",
      storage: createJSONStorage(() => safeLocalStorage),
    },
  ),
);

/** SendFile from an absolute path. */
export function sendFileFromPath(p: string): SendFile {
  const name = baseName(p);
  return { path: p, name, ext: extOf(name) };
}
