/* ── overlays (send flow, drawers, modals) ──────────────────────────────── */

import { create } from "zustand";
import type { IncomingRequest } from "../../bridge/api";
import type { SendFile } from "./recents";

export type SendStep = "files" | "device" | "confirm" | "waiting";

export type PendingRow = {
  deviceId: string;
  name: string;
  sessionId?: string;
  sas?: string;
  ok: boolean;
  failed?: string;
};

export type SendState = {
  step: SendStep;
  preset: boolean;
  deviceIds: string[];
  /** file pool shown in step 1 (recents ∪ freshly picked) */
  pool: SendFile[];
  /** selected file keys (path ?? name) */
  sel: string[];
  pending: PendingRow[];
  startedTrusted: number;
};

/** What opens the pairing form on its JOIN view: a pasted/opened
 *  `lanbeam://pair` link (unpacked by the form), or just an address — with the
 *  device's name when the caller knows it (the fingerprint alert does). */
export type PairPrefill = { link: string } | { addr: string; name?: string };

/** The remembered device whose name has reappeared under a different key.
 *
 *  It used to carry a `step` and a `sas`, for a "re-verify and restore trust"
 *  flow. That flow dialled the new key, showed the SAS it produced, and asked the
 *  user to confirm it matched — while the OTHER device displayed nothing at all.
 *  There was no second number. Trusting an unknown key is what pairing is for,
 *  and pairing shows the code on both screens; the alert now sends you there. */
export type FpAlert = {
  deviceId: string;
};

/** A parked name-collision awaiting the ConflictModal's decision (M6.5). The
 *  incoming card already verified the SAS; the modal issues the single reply
 *  (accept + keep-both/overwrite, or decline), so it also carries whether the
 *  user asked to trust the peer, applied only on a positive choice. */
export type PendingConflict = {
  request: IncomingRequest;
  peerName: string;
  wantTrust: boolean;
};

interface OverlayState {
  send: SendState | null;
  /** A send dismissed by a click outside while still being set up (files →
   *  device → confirm), kept so that reopening resumes at that step with the
   *  same files and devices instead of starting over — a stray click shouldn't
   *  cost the selection. Gone once the send goes out, on the explicit ×, and
   *  when a send is started for specific files (a drop, "send again") or for
   *  another device. Memory only: a restart starts clean. */
  sendDraft: SendState | null;
  detailId: string | null;
  pairOpen: boolean;
  /** What to pre-fill the pairing form's JOIN view with, staged by whoever
   *  opened it: a `lanbeam://pair` link (deep link), or a bare address (a
   *  `lanbeam://connect` link, the fingerprint alert). A fresh object per call:
   *  the form applies each object once, by identity, so a second link reaches
   *  an already-open form even when it is the same link. Cleared on close. */
  pairPrefill: PairPrefill | null;
  qtOpen: boolean;
  /** Body staged for quick-text by a `lanbeam://text?t=…` deep link. Cleared
   *  once consumed / on close. The user still picks a device and sends. */
  qtPrefill: string | null;
  shareOpen: boolean;
  licenseOpen: boolean;
  fpAlert: FpAlert | null;
  conflict: PendingConflict | null;
  dragOver: boolean;
  dragDevice: string | null;
  openSend: (
    deviceId: string | null,
    pool: SendFile[],
    presel?: SendFile[],
  ) => void;
  patchSend: (p: Partial<SendState>) => void;
  /** Close and forget: the send went out, or the user pressed ×. */
  closeSend: () => void;
  /** Close but keep the selection as `sendDraft` (a click outside). */
  dismissSend: () => void;
  setDetail: (id: string | null) => void;
  setPair: (v: boolean, prefill?: PairPrefill | null) => void;
  setQt: (v: boolean, prefill?: string | null) => void;
  setShare: (v: boolean) => void;
  setLicense: (v: boolean) => void;
  setFpAlert: (v: FpAlert | null) => void;
  setConflict: (v: PendingConflict | null) => void;
  setDrag: (over: boolean, device?: string | null) => void;
}

export const useOverlays = create<OverlayState>((set) => ({
  send: null,
  sendDraft: null,
  detailId: null,
  pairOpen: false,
  pairPrefill: null,
  qtOpen: false,
  qtPrefill: null,
  shareOpen: false,
  licenseOpen: false,
  fpAlert: null,
  conflict: null,
  dragOver: false,
  dragDevice: null,
  openSend: (deviceId, pool, presel) =>
    set((s) => {
      const d = s.sendDraft;
      // Resume the draft when the send is reopened the way it was started:
      // plainly (no files handed in), and for the same device if one was
      // clicked. Files handed in, or another device, are a new send.
      const resume =
        d &&
        !presel?.length &&
        (!deviceId ||
          (d.preset &&
            d.deviceIds.length === 1 &&
            d.deviceIds[0] === deviceId));
      if (resume) {
        // Recents may have grown since; keep the draft's own files first.
        const known = new Set(d.pool.map((f) => f.path ?? f.name));
        return {
          send: {
            ...d,
            pool: [
              ...d.pool,
              ...pool.filter((f) => !known.has(f.path ?? f.name)),
            ],
          },
          sendDraft: null,
        };
      }
      return {
        sendDraft: null,
        send: {
          step: "files",
          preset: !!deviceId,
          deviceIds: deviceId ? [deviceId] : [],
          pool: [
            ...(presel ?? []),
            ...pool.filter(
              (f) =>
                !(presel ?? []).some(
                  (p) => (p.path ?? p.name) === (f.path ?? f.name),
                ),
            ),
          ],
          sel: (presel ?? []).map((f) => f.path ?? f.name),
          pending: [],
          startedTrusted: 0,
        },
      };
    }),
  patchSend: (p) => set((s) => (s.send ? { send: { ...s.send, ...p } } : s)),
  closeSend: () => set({ send: null, sendDraft: null }),
  // Once the requests are out ("waiting") there is nothing left to resume —
  // they are with the peers — so only a send still being set up is kept.
  dismissSend: () =>
    set((s) => ({
      send: null,
      sendDraft: s.send && s.send.step !== "waiting" ? s.send : null,
    })),
  setDetail: (detailId) => set({ detailId }),
  // Opening with a prefill stages it for the form; closing (or opening without
  // one) clears it so a later manual open is clean. The copy makes every call a
  // new object, which is what tells an ALREADY-open form to look again.
  setPair: (pairOpen, prefill = null) =>
    set({
      pairOpen,
      pairPrefill: pairOpen && prefill ? { ...prefill } : null,
    }),
  setQt: (qtOpen, prefill = null) =>
    set({ qtOpen, qtPrefill: qtOpen ? prefill : null }),
  setShare: (shareOpen) => set({ shareOpen }),
  setLicense: (licenseOpen) => set({ licenseOpen }),
  setFpAlert: (fpAlert) => set({ fpAlert }),
  setConflict: (conflict) => set({ conflict }),
  setDrag: (dragOver, dragDevice = null) => set({ dragOver, dragDevice }),
}));
