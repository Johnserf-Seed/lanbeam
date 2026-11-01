/* ── live browser shares ────────────────────────────────────────────────── */

import { create } from "zustand";
import * as api from "../../bridge/api";
import type { ShareEntry } from "../../bridge/api";

/** The browser shares serving files RIGHT NOW.
 *
 *  A live share is files being served over HTTP on your LAN, and until now the
 *  only place that fact appeared was inside the modal that created it. Closing
 *  that modal does not stop the share — deliberately, because a link you handed
 *  someone should survive you closing the panel you copied it from — so a share
 *  you'd forgotten went on serving, invisible AND unstoppable: reopening the modal
 *  minted a NEW share rather than adopting the live one, so the only 停止分享
 *  button in the app could never reach the old one again.
 *
 *  Backend-owned and event-fed (`shares_updated` fires on start / update / stop,
 *  on a download that exhausts a budget, and from the sweeper when a share expires
 *  on its own) — the indicator has to go out by itself, not the next time somebody
 *  happens to open a modal. */
interface SharesState {
  shares: ShareEntry[];
  setShares: (shares: ShareEntry[]) => void;
  load: () => Promise<void>;
}

export const useShares = create<SharesState>((set) => ({
  shares: [],
  setShares: (shares) => set({ shares }),
  load: async () => {
    const shares = await api.listShares().catch(() => [] as ShareEntry[]);
    set({ shares });
  },
}));
