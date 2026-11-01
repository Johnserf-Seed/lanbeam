/* ── transfers ──────────────────────────────────────────────────────────── */

import { create } from "zustand";
import {
  persist,
  type PersistStorage,
  type StorageValue,
} from "zustand/middleware";
import type { IncomingRequest, TransferErrorCode } from "../../bridge/api";
import { extOf } from "../format";
import { keepRecentTexts, safeLocalStorage } from "./persist";
import { usePrefs } from "./prefs";

// "queued" = accepted/started but parked on the concurrency gate (M6.7); a
// transient in-progress state the transfer_started/progress events clear.
export type UITransferStatus = "active" | "queued" | "done" | "error";

export type UITransfer = {
  sessionId: string;
  /** absent or "file" = a real file transfer (the default); "text" = a
   *  lightweight quick-text history entry (M7.3) — no bytes, no drawer. */
  kind?: "file" | "text";
  /** the message body for a "text" record (the whole quick text) */
  text?: string;
  /** set on a browser-share download record: a browser pulled a shared file via
   *  the link (not a paired device). Renders like a done "send" but opens no
   *  detail drawer (there is no session/paths); `peerName` carries the IP. */
  via?: "browser";
  direction: "send" | "receive";
  totalSize: number;
  fileCount?: number;
  percent: number;
  status: UITransferStatus;
  error?: string;
  /** machine-readable cause from transfer_error — known codes translate at
   *  render time, so the row isn't frozen in the locale of the moment */
  errorCode?: TransferErrorCode;
  sas?: string;
  savedNames?: string[];
  /** transfer_started arrived (the connection is open — NOT that the peer
   *  accepted; on a send the receiver's prompt may still be up) */
  started?: boolean;
  /** SEND only: the receiver accepted (`transfer_accepted`). Until then the row
   *  is waiting on a person, and the SAS is what both screens should show. */
  accepted?: boolean;
  peerId?: string;
  peerName?: string;
  /** display name: first file (+ n more handled by the page) */
  name?: string;
  ext?: string;
  files?: { name: string; size?: number }[];
  /** outgoing source paths (再次发送) */
  paths?: string[];
  /** paused via pause_transfer (M6.2). Session-local backpressure: the UI sets it
   *  optimistically on click. It is NOT the UI's alone to clear — the backend's
   *  pause is bounded, and when the park expires it resumes on its own and emits
   *  `transfer_resumed`, which AppShell listens for. (This comment used to say
   *  "the backend emits no state event"; it had stopped being true, and nothing
   *  listened, so a resumed transfer streamed bytes under a row that still read
   *  「已暂停」 until the user clicked a 继续 button that did nothing.) */
  paused?: boolean;
  /** live per-file state (M6.8) keyed by manifest file index, fed by the
   *  transfer_file_progress / transfer_file_done events. Absent → the detail
   *  drawer falls back to its cumulative-size estimate. */
  fileStat?: Record<
    number,
    { percent: number; done: boolean; verified: boolean }
  >;
  speedBps: number;
  /** MB/s samples, ≤40, for sparkline + speed curve */
  hist: number[];
  startedAt: number;
  doneAt?: number;
  /* internals for speed sampling */
  _pm?: number;
  _tm?: number;
};

export type OutgoingMeta = {
  peerId: string;
  peerName: string;
  name: string;
  ext: string;
  files: { name: string; size?: number }[];
  paths: string[];
};

interface TransfersState {
  transfers: Record<string, UITransfer>;
  incomings: IncomingRequest[];
  /** sessionId → what the UI sent under it (H11). The UI chooses the id and
   *  hands it to send_files, so the record joins exactly that session's row on
   *  its first event — queued, SAS or started. It used to be a FIFO per device,
   *  taken by the next sas_code: a send that failed before one (peer offline,
   *  unreadable file, cancelled while queued) left its record for the NEXT send
   *  to that device, and 再次发送 on that row resent the wrong files. */
  pendingSend: Record<string, OutgoingMeta>;
  /** sessionId → meta for an accepted incoming request */
  pendingRecv: Record<
    string,
    Omit<OutgoingMeta, "paths" | "peerId"> & { peerId: string }
  >;
  upsert: (t: Partial<UITransfer> & { sessionId: string }) => void;
  /** insert a lightweight quick-text history record (M7.3) — a terminal,
   *  file-less entry that surfaces in the Transfers history list. */
  addTextTransfer: (p: {
    direction: "send" | "receive";
    peerId: string;
    peerName: string;
    text: string;
  }) => void;
  /** record a browser-share download as a persistent history entry — a browser
   *  pulled a shared file via the link (M8.4). */
  addShareDownload: (p: { name: string; size: number; peerIp: string }) => void;
  progress: (sessionId: string, percent: number, totalSize?: number) => void;
  /** flip the session-local pause flag (M6.2); a no-op for an unknown session */
  setPaused: (sessionId: string, paused: boolean) => void;
  /** record a per-file progress tick (M6.8) — keyed by manifest file index */
  fileProgress: (sessionId: string, fileIndex: number, percent: number) => void;
  /** mark a per-file completion (M6.8); `verified` shows the ✓ tick */
  fileDone: (sessionId: string, fileIndex: number, verified: boolean) => void;
  registerOutgoing: (sessionId: string, meta: OutgoingMeta) => void;
  /** Forget a send's record that no event will claim (the send failed first). */
  dropOutgoing: (sessionId: string) => void;
  attachSas: (sessionId: string, sas: string, deviceId?: string) => void;
  pushIncoming: (r: IncomingRequest) => void;
  shiftIncoming: () => void;
  /** drop a specific prompt card (e.g. its session already errored out) */
  removeIncoming: (sessionId: string) => void;
  acceptMeta: (r: IncomingRequest, peerName: string) => void;
  removeTransfer: (sessionId: string) => void;
  /** Drop finished rows that have outlived 历史记录保留. */
  pruneHistory: () => void;
}

// Monotonic counter for quick-text record ids. Combined with a timestamp and a
// random suffix under a distinctive "text-" prefix, it guarantees a text
// record's sessionId can never collide with a backend transfer_id (a UUID) or
// with another text record minted in the same millisecond.
let textRecordSeq = 0;

/** The row fields a send's record fills in when its session's first event
 *  arrives. */
function outgoingFields(m: OutgoingMeta): Partial<UITransfer> {
  return {
    peerId: m.peerId,
    peerName: m.peerName,
    name: m.name,
    ext: m.ext,
    files: m.files,
    paths: m.paths,
  };
}

/** The shortest stretch a speed sample spans (L26). */
const SPEED_SAMPLE_MS = 250;

/** Persisted shape of useTransfers (see partialize below): terminal rows only. */
type PersistedTransfers = { transfers: Record<string, UITransfer> };

/** How long a finished transfer stays in 历史, per 设置 → 历史记录保留.
 *
 *  This setting had a dropdown, a persisted value, a description promising
 *  「超期的传输记录自动清除」 — and not one reader anywhere in the codebase. The
 *  only trimming that ever happened was a 200-row cap, which is a count, not a
 *  time; picking 「不保留」 still wrote every row to disk.
 *
 *  `none` → nothing survives a restart (the live list is untouched — you still
 *  watch this session's transfers finish). `forever` → no expiry. */
export function histWindowMs(keep: string): number {
  switch (keep) {
    case "none":
      return 0;
    case "7d":
      return 7 * 24 * 3600_000;
    case "30d":
      return 30 * 24 * 3600_000;
    default:
      return Number.POSITIVE_INFINITY;
  }
}

/** A localStorage-backed persist storage that skips the write when the
 *  partialized (terminal-only) snapshot is unchanged. The backend throttles
 *  progress to whole-percent steps, so a single multi-GB / multi-file transfer
 *  fires hundreds–thousands of transfer_progress + transfer_file_progress
 *  events, and zustand's persist would run JSON.stringify + a synchronous
 *  localStorage.setItem of the whole ~100 KB history blob on EVERY one — pure
 *  wasted main-thread work, since progress ticks never touch a terminal row.
 *
 *  partialize rebuilds the map each call but leaves untouched records' object
 *  identities intact, so reference-comparing the record list is an exact,
 *  O(rows) change detector. It's loss-free (no debounce window): the instant a
 *  row turns done/error, addTextTransfer runs, or a row is removed, the record
 *  set changes and the write goes through synchronously as before. The on-disk
 *  format is identical to createJSONStorage's ({state[,version]} JSON under the
 *  same key), so existing history hydrates with no migration. */
function terminalDiffStorage(): PersistStorage<PersistedTransfers> {
  let lastRecords: UITransfer[] | null = null;
  return {
    getItem: (name) => {
      const str = safeLocalStorage.getItem(name) as string | null;
      if (!str) return null;
      try {
        return JSON.parse(str) as StorageValue<PersistedTransfers>;
      } catch {
        return null; // a corrupt blob is no history, not a crash
      }
    },
    setItem: (name, value) => {
      const entries = Object.entries(value.state.transfers);
      const records = entries.map(([, r]) => r);
      if (
        lastRecords &&
        lastRecords.length === records.length &&
        records.every((r, i) => r === lastRecords?.[i])
      ) {
        return; // nothing terminal changed — skip serialize + write
      }
      lastRecords = records;
      // partialize ordered the rows newest first; texts past the budget are
      // saved without their bodies (M23). After the identity check above, so
      // the copies it makes never defeat it.
      const kept = keepRecentTexts(records);
      const transfers = Object.fromEntries(
        entries.map(([key], i) => [key, kept[i]]),
      );
      safeLocalStorage.setItem(
        name,
        JSON.stringify({ ...value, state: { transfers } }),
      );
    },
    removeItem: (name) => {
      // The next write must not be skipped as "unchanged" — there is nothing on
      // disk to be unchanged from.
      lastRecords = null;
      safeLocalStorage.removeItem(name);
    },
  };
}

/** When this run of the app started: what tells the transfers it finishes from
 *  the history restored at launch (M22). */
const SESSION_START = Date.now();

export const useTransfers = create<TransfersState>()(
  persist(
    (set) => ({
      transfers: {},
      incomings: [],
      pendingSend: {},
      pendingRecv: {},
      upsert: (t) =>
        set((s) => {
          const prev: UITransfer = s.transfers[t.sessionId] ?? {
            sessionId: t.sessionId,
            direction: t.direction ?? "receive",
            totalSize: 0,
            percent: 0,
            status: "active",
            speedBps: 0,
            hist: [],
            startedAt: Date.now(),
          };
          const merged = { ...prev, ...t };
          // A send we started: its first event carries what we sent (H11) —
          // a queued row included, which used to sit there nameless.
          const om = s.pendingSend[t.sessionId];
          let pendingSend = s.pendingSend;
          if (om) {
            Object.assign(merged, outgoingFields(om), { direction: "send" });
            pendingSend = { ...pendingSend };
            delete pendingSend[t.sessionId];
          }
          // Attach pending receive meta when a receive transfer starts.
          const rm = s.pendingRecv[t.sessionId];
          if (rm && !merged.peerName) {
            merged.peerId = rm.peerId;
            merged.peerName = rm.peerName;
            merged.name = rm.name;
            merged.ext = rm.ext;
            merged.files = rm.files;
          }
          if ((t.status === "done" || t.status === "error") && !merged.doneAt) {
            merged.doneAt = Date.now();
            merged.speedBps = 0;
          }
          return {
            transfers: { ...s.transfers, [t.sessionId]: merged },
            ...(om ? { pendingSend } : {}),
          };
        }),
      addTextTransfer: ({ direction, peerId, peerName, text }) =>
        set((s) => {
          const preview = text.replace(/\s+/g, " ").trim();
          const now = Date.now();
          textRecordSeq += 1;
          const suffix = Math.random().toString(36).slice(2, 8);
          const sessionId = `text-${now}-${textRecordSeq}-${suffix}`;
          const rec: UITransfer = {
            sessionId,
            kind: "text",
            text,
            direction,
            peerId,
            peerName,
            name: preview.length > 48 ? `${preview.slice(0, 48)}…` : preview,
            ext: "TXT",
            totalSize: 0,
            fileCount: 1,
            percent: 100,
            status: "done",
            speedBps: 0,
            hist: [],
            startedAt: now,
            doneAt: now,
          };
          return { transfers: { ...s.transfers, [sessionId]: rec } };
        }),
      addShareDownload: ({ name, size, peerIp }) =>
        set((s) => {
          const now = Date.now();
          textRecordSeq += 1;
          const suffix = Math.random().toString(36).slice(2, 8);
          const sessionId = `share-${now}-${textRecordSeq}-${suffix}`;
          const rec: UITransfer = {
            sessionId,
            kind: "file",
            via: "browser",
            direction: "send",
            // No paired device — the downloader is an anonymous browser; its IP
            // stands in for the peer name.
            peerName: peerIp,
            name,
            ext: extOf(name),
            totalSize: size,
            fileCount: 1,
            percent: 100,
            status: "done",
            speedBps: 0,
            hist: [],
            startedAt: now,
            doneAt: now,
          };
          return { transfers: { ...s.transfers, [sessionId]: rec } };
        }),
      progress: (sessionId, percent, totalSize) =>
        set((s) => {
          const prev = s.transfers[sessionId];
          if (!prev) return s;
          // A straggler progress tick must never resurrect a terminal row —
          // loopback send-to-self reuses one sessionId for both directions, so
          // the send task's still-queued ticks can land after the receive task
          // already marked the row done/error and would otherwise flip it back
          // to "active" (and, on an app exit in that window, drop it from
          // history since partialize persists only terminal rows). The
          // queued→active promotion (M6.7) below is untouched.
          if (prev.status === "done" || prev.status === "error") return s;
          const now = Date.now();
          const total = totalSize ?? prev.totalSize;
          let speedBps = prev.speedBps;
          let hist = prev.hist;
          // Speed is measured over at least SPEED_SAMPLE_MS (L26). Progress
          // comes per whole percent, and a burst of events can land in one
          // task a millisecond apart — dividing by that millisecond made
          // spikes, and 「峰值」 figures, of gigabytes a second.
          let mark = { _pm: prev._pm, _tm: prev._tm };
          if (prev._tm === undefined || prev._pm === undefined) {
            mark = { _pm: percent, _tm: now };
          } else if (now - prev._tm >= SPEED_SAMPLE_MS) {
            if (percent > prev._pm) {
              const dBytes = (total * (percent - prev._pm)) / 100;
              speedBps = dBytes / ((now - prev._tm) / 1000);
              hist = [...hist.slice(-39), speedBps / 1048576];
            }
            mark = { _pm: percent, _tm: now };
          }
          return {
            transfers: {
              ...s.transfers,
              [sessionId]: {
                ...prev,
                percent,
                totalSize: total,
                status: "active",
                speedBps,
                hist,
                ...mark,
              },
            },
          };
        }),
      setPaused: (sessionId, paused) =>
        set((s) => {
          const prev = s.transfers[sessionId];
          if (!prev) return s;
          // Paused moves nothing: no speed, so no ETA (L26). Resuming starts a
          // fresh sample, or the first one after would average in the pause.
          const speed = paused
            ? { speedBps: 0 }
            : { _pm: prev.percent, _tm: Date.now() };
          return {
            transfers: {
              ...s.transfers,
              [sessionId]: { ...prev, paused, ...speed },
            },
          };
        }),
      fileProgress: (sessionId, fileIndex, percent) =>
        set((s) => {
          const prev = s.transfers[sessionId];
          if (!prev) return s;
          const fileStat = { ...(prev.fileStat ?? {}) };
          const cur = fileStat[fileIndex] ?? {
            percent: 0,
            done: false,
            verified: false,
          };
          // A late progress tick must never un-finish a file already marked done.
          if (cur.done) return s;
          fileStat[fileIndex] = { ...cur, percent };
          return {
            transfers: { ...s.transfers, [sessionId]: { ...prev, fileStat } },
          };
        }),
      fileDone: (sessionId, fileIndex, verified) =>
        set((s) => {
          const prev = s.transfers[sessionId];
          if (!prev) return s;
          const fileStat = { ...(prev.fileStat ?? {}) };
          fileStat[fileIndex] = { percent: 100, done: true, verified };
          return {
            transfers: { ...s.transfers, [sessionId]: { ...prev, fileStat } },
          };
        }),
      registerOutgoing: (sessionId, meta) =>
        set((s) => ({ pendingSend: { ...s.pendingSend, [sessionId]: meta } })),
      dropOutgoing: (sessionId) =>
        set((s) => {
          if (!s.pendingSend[sessionId]) return s;
          const pendingSend = { ...s.pendingSend };
          delete pendingSend[sessionId];
          return { pendingSend };
        }),
      attachSas: (sessionId, sas, deviceId) =>
        set((s) => {
          // Exactly this session's record, if the UI started it (H11).
          const meta = s.pendingSend[sessionId];
          let pendingSend = s.pendingSend;
          if (meta) {
            pendingSend = { ...pendingSend };
            delete pendingSend[sessionId];
          }
          const prev: UITransfer = s.transfers[sessionId] ?? {
            sessionId,
            direction: "send",
            totalSize: 0,
            percent: 0,
            status: "active",
            speedBps: 0,
            hist: [],
            startedAt: Date.now(),
          };
          return {
            pendingSend,
            transfers: {
              ...s.transfers,
              [sessionId]: {
                ...prev,
                sas,
                peerId: deviceId ?? prev.peerId,
                ...(meta ? outgoingFields(meta) : {}),
              },
            },
          };
        }),
      // Once per session: a request fetched at startup (L23) can also arrive
      // as its event.
      pushIncoming: (r) =>
        set((s) =>
          s.incomings.some((x) => x.sessionId === r.sessionId)
            ? s
            : { incomings: [...s.incomings, r] },
        ),
      shiftIncoming: () => set((s) => ({ incomings: s.incomings.slice(1) })),
      removeIncoming: (sessionId) =>
        set((s) =>
          s.incomings.some((r) => r.sessionId === sessionId)
            ? {
                incomings: s.incomings.filter((r) => r.sessionId !== sessionId),
              }
            : s,
        ),
      acceptMeta: (r, peerName) =>
        set((s) => ({
          pendingRecv: {
            ...s.pendingRecv,
            [r.sessionId]: {
              peerId: r.deviceId,
              peerName,
              name: r.files[0]?.name ?? "",
              ext: extOf(r.files[0]?.name ?? ""),
              files: r.files.map((f) => ({ name: f.name, size: f.size })),
            },
          },
        })),
      removeTransfer: (sessionId) =>
        set((s) => {
          const transfers = { ...s.transfers };
          delete transfers[sessionId];
          return { transfers };
        }),
      // partialize only trims on the next WRITE, so without this an expired row
      // lingers in the live list until something else happens to persist. Run at
      // startup and whenever 历史记录保留 changes.
      //
      // 「不保留」 means "don't KEEP them", not "delete the ones you are watching
      // right now": what it drops is the history restored from earlier runs. It
      // used to return before doing anything — so the old history stayed on
      // disk until some transfer happened to finish, and came back on every
      // restart until then (M22). Now that history goes, and the `set` below
      // rewrites the saved copy through partialize — empty under 「不保留」 —
      // whether or not anything on screen changed.
      pruneHistory: () => {
        const win = histWindowMs(usePrefs.getState().histKeep);
        if (!Number.isFinite(win)) return;
        const cutoff = win === 0 ? SESSION_START : Date.now() - win;
        set((s) => {
          const kept = Object.entries(s.transfers).filter(
            ([, t]) =>
              (t.status !== "done" && t.status !== "error") ||
              (t.doneAt ?? 0) >= cutoff,
          );
          if (kept.length === Object.keys(s.transfers).length) return s;
          return { transfers: Object.fromEntries(kept) };
        });
      },
    }),
    {
      name: "lanbeam.transfers",
      // Skip the redundant serialize+write that progress ticks would otherwise
      // trigger on this hot path (see terminalDiffStorage) — the persisted set
      // only changes on terminal transitions.
      storage: terminalDiffStorage(),
      // Persist only terminal transfers (history survives restarts). "active"
      // AND "queued" are live in-progress states — never persist them, or a
      // restart mid-transfer would resurrect a frozen row that no event clears.
      // Then apply 历史记录保留 (histWindowMs) — the setting that, until now,
      // nothing read: 「不保留」 wrote every row to disk anyway.
      partialize: (s) => {
        const win = histWindowMs(usePrefs.getState().histKeep);
        // 「不保留」: nothing survives the restart. The live list is untouched —
        // you still watch this session's transfers finish; they just aren't kept.
        if (win === 0) return { transfers: {} };
        const cutoff = Date.now() - win; // -Infinity under 「永久保留」
        return {
          transfers: Object.fromEntries(
            Object.entries(s.transfers)
              .filter(([, t]) => t.status === "done" || t.status === "error")
              .filter(([, t]) => (t.doneAt ?? 0) >= cutoff)
              .sort((a, b) => (b[1].doneAt ?? 0) - (a[1].doneAt ?? 0))
              .slice(0, 200),
          ),
        };
      },
    },
  ),
);

/** Ordered list helpers. */
export function transferList(map: Record<string, UITransfer>): UITransfer[] {
  return Object.values(map).sort((a, b) => b.startedAt - a.startedAt);
}
