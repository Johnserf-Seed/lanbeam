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
          const seen = new Set(files.map((f) => f.path ?? f.name));
          const rest = s.items.filter((f) => !seen.has(f.path ?? f.name));
          return { items: [...files, ...rest].slice(0, 8) };
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
