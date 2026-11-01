/* ── inbox ──────────────────────────────────────────────────────────────── */

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import type { FileCat } from "../filecat";
import { fileCat } from "../filecat";
import { extOf } from "../format";
import { keepRecentTexts, safeLocalStorage } from "./persist";
import type { UITransfer } from "./transfers";

export type InboxItem = {
  id: string;
  kind: FileCat | "txt";
  ext: string;
  name: string;
  from: string;
  ts: number;
  sizeBytes: number;
  count: number;
  sessionId?: string;
  paths?: string[];
  text?: string;
};

interface InboxState {
  items: InboxItem[];
  unread: number;
  add: (item: InboxItem) => void;
  remove: (ids: string[]) => void;
  clearUnread: () => void;
}

export const useInbox = create<InboxState>()(
  persist(
    (set) => ({
      items: [],
      unread: 0,
      add: (item) =>
        // Dedupe by id: a session that emits its completion more than once (e.g.
        // loopback send-to-self, where one transfer_id keys both directions)
        // must replace, not duplicate, the record.
        set((s) => {
          const dup = s.items.some((i) => i.id === item.id);
          return {
            items: [item, ...s.items.filter((i) => i.id !== item.id)].slice(
              0,
              500,
            ),
            unread: dup ? s.unread : s.unread + 1,
          };
        }),
      remove: (ids) =>
        set((s) => ({ items: s.items.filter((i) => !ids.includes(i.id)) })),
      clearUnread: () => set({ unread: 0 }),
    }),
    {
      name: "lanbeam.inbox",
      storage: createJSONStorage(() => safeLocalStorage),
      partialize: (s) => ({
        items: keepRecentTexts(s.items),
        unread: s.unread,
      }),
    },
  ),
);

/** Build an inbox item from a completed inbound transfer. */
export function inboxFromTransfer(t: UITransfer, paths: string[]): InboxItem {
  const first = t.savedNames?.[0] ?? t.files?.[0]?.name ?? t.name ?? "";
  const n = t.fileCount ?? t.files?.length ?? t.savedNames?.length ?? 1;
  // Store only the first file name; multi-file display names are composed at
  // render time from `count` (transfers.filesMore) so they follow the locale.
  const name = first;
  const ext = extOf(first);
  return {
    id: t.sessionId,
    kind: fileCat(ext),
    ext,
    name,
    from: t.peerName ?? "",
    ts: Date.now(),
    sizeBytes: t.totalSize,
    count: n,
    sessionId: t.sessionId,
    paths,
  };
}

/** Build an inbox item from a received quick text (M7.3). The display name is a
 *  single-line preview of the content — stored raw (not locale-prefixed) so it
 *  never freezes in the language of the moment; the ExtChip's “” glyph already
 *  marks it as text. `at` is the backend's millisecond receive stamp. */
export function inboxFromText(
  from: string,
  text: string,
  at: number,
): InboxItem {
  const preview = text.replace(/\s+/g, " ").trim();
  return {
    id: `txt-${at}-${Math.random().toString(36).slice(2, 8)}`,
    kind: "txt",
    ext: "TXT",
    name: preview.length > 48 ? `${preview.slice(0, 48)}…` : preview,
    from,
    ts: at,
    // Texts never touch disk; the row shows a char count, not a byte size.
    sizeBytes: 0,
    count: 1,
    text,
  };
}
