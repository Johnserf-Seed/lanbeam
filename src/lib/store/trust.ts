/* ── trust circle ───────────────────────────────────────────────────────── */

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import * as api from "../../bridge/api";
import type { DiscoveredDevice, TrustedPeer } from "../../bridge/api";
import { useData } from "./data";
import { safeLocalStorage } from "./persist";

export type TrustRecord = {
  deviceId: string;
  /** what the device calls itself — refreshed by every discovery and every
   *  transfer it sends, and what the fingerprint-changed check compares */
  name: string;
  /** the user's own name for it (a rename): shown instead of `name`, and kept
   *  apart from it so those refreshes can't undo the rename (M7). Local to this
   *  machine. */
  alias?: string;
  trusted: boolean;
  autoAccept: boolean;
  addedAt: number;
  lastSeen: number;
  /** free-mode position on the 640×430 base canvas */
  pos?: { x: number; y: number };
};

/** The name to show for a remembered device: the user's rename, else its own. */
export function trustName(r: TrustRecord | undefined): string | undefined {
  return r?.alias || r?.name || undefined;
}

/** A trust-page row: union of remembered records and live discoveries. */
export type TrustDevice = {
  deviceId: string;
  name: string;
  trusted: boolean;
  autoAccept: boolean;
  online: boolean;
  /** short fingerprint for display (derived from deviceId) */
  fp: string;
  address?: string;
  /** same name reappeared under a different key while the old one is gone */
  fpChanged?: { newDeviceId: string };
  pos?: { x: number; y: number };
};

interface TrustState {
  records: Record<string, TrustRecord>;
  sel: string | null;
  /** legacy localStorage records were pushed to the backend once (M4.4) */
  migrated: boolean;
  setSel: (id: string | null) => void;
  setTrust: (d: { deviceId: string; name: string }, trusted: boolean) => void;
  toggleAuto: (deviceId: string) => void;
  remove: (deviceId: string) => void;
  restore: (rec: TrustRecord) => void;
  rename: (deviceId: string, name: string) => void;
  setPos: (deviceId: string, pos: { x: number; y: number }) => void;
  touch: (deviceId: string, name: string) => void;
  /** trust the new key of a device whose fingerprint changed */
  /** adopt the backend list (list_trusted / trust_updated) as the truth */
  hydrate: (list: TrustedPeer[]) => void;
}

// Fire-and-forget backend writes: the UI already updated optimistically and
// the authoritative list comes back via `trust_updated`, so a failed IPC call
// self-corrects on the next event instead of surfacing an error.
const pushTrust = (
  deviceId: string,
  name: string,
  autoAccept: boolean,
): void => {
  void api.setTrusted(deviceId, name, autoAccept).catch(() => {});
};
const dropTrust = (deviceId: string): void => {
  void api.removeTrusted(deviceId).catch(() => {});
};

/** Trust records are backend-backed since M4.4: every mutation is applied
 *  optimistically AND written through to the Rust trust store (which is what
 *  actually decides auto-accept). Records with `trusted: false` are pure UI
 *  memory (a remembered name/position for an untrusted device) — the backend
 *  has no such concept, so they stay local and survive hydration. */
export const useTrust = create<TrustState>()(
  persist(
    (set, get) => ({
      records: {},
      sel: null,
      migrated: false,
      setSel: (sel) => set({ sel }),
      setTrust: (d, trusted) => {
        const prev = get().records[d.deviceId];
        const rec: TrustRecord = {
          deviceId: d.deviceId,
          name: prev?.name ?? d.name,
          alias: prev?.alias,
          trusted,
          // Trusting a device also turns on auto-accept by default — "these are
          // my devices, stop nagging me". A device that was ALREADY trusted keeps
          // its explicit auto-accept choice (so turning it OFF for one device
          // sticks); a fresh trust defaults ON. Untrusting always clears it.
          autoAccept: trusted
            ? prev?.trusted
              ? prev.autoAccept
              : true
            : false,
          addedAt: prev?.addedAt ?? Date.now(),
          lastSeen: Date.now(),
          pos: prev?.pos,
        };
        set((s) => ({ records: { ...s.records, [d.deviceId]: rec } }));
        // Untrusting keeps the local memo but must delete the backend row —
        // an entry over there is what auto-accept consults.
        if (trusted) pushTrust(rec.deviceId, rec.name, rec.autoAccept);
        else dropTrust(d.deviceId);
      },
      toggleAuto: (id) => {
        const r = get().records[id];
        if (!r) return;
        const next = { ...r, autoAccept: !r.autoAccept };
        set((s) => ({ records: { ...s.records, [id]: next } }));
        if (next.trusted) pushTrust(id, next.name, next.autoAccept);
      },
      // 删除设备 — the strong one. Clears the trust row AND the manually-added
      // address (IP-direct / pair-by-code) that keeps the peer in the device
      // list. `dropTrust` alone was never enough: the trust row went, the manual
      // address stayed, and the device was back on the very next list with
      // nothing able to remove it. It also drops the local memo (name, ring
      // position) — deleting a device means forgetting all of it.
      //
      // A device still announcing itself on the LAN of course comes back. That
      // is discovery, not a failure to delete: this erases LanBeam's memory of
      // the device, not the machine. The caller says which of the two happened.
      remove: (id) => {
        set((s) => {
          const records = { ...s.records };
          delete records[id];
          return { records, sel: s.sel === id ? null : s.sel };
        });
        // Drop it from the live list too, so it disappears NOW rather than on the
        // next devices_updated (and, for a manual peer, never comes back).
        useData.setState((s) => ({
          devices: s.devices.filter((d) => d.deviceId !== id),
        }));
        void api.forgetDevice(id).catch(() => {});
      },
      restore: (rec) => {
        set((s) => ({ records: { ...s.records, [rec.deviceId]: rec } }));
        if (rec.trusted) pushTrust(rec.deviceId, rec.name, rec.autoAccept);
      },
      // A rename is an alias, local to this machine (M7). It used to overwrite
      // `name` — which the next discovery event or received transfer refreshed
      // straight back to whatever the device calls itself. It is not written to
      // the backend either: the backend's name is the device's own, and the
      // backend rewrites it on every transfer that device sends. Renaming back
      // to the device's own name clears the alias.
      rename: (id, alias) => {
        const prev = get().records[id];
        const own =
          prev?.name ??
          useData.getState().devices.find((d) => d.deviceId === id)?.name ??
          alias;
        const rec: TrustRecord = {
          ...(prev ?? {
            deviceId: id,
            trusted: false,
            autoAccept: false,
            addedAt: Date.now(),
            lastSeen: Date.now(),
          }),
          name: own,
          alias: alias === own ? undefined : alias,
        };
        set((s) => ({ records: { ...s.records, [id]: rec } }));
      },
      setPos: (id, pos) =>
        set((s) => {
          const r = s.records[id];
          if (!r) return s;
          return { records: { ...s.records, [id]: { ...r, pos } } };
        }),
      touch: (id, name) =>
        set((s) => {
          const r = s.records[id];
          if (!r) return s;
          return {
            records: {
              ...s.records,
              [id]: { ...r, name: name || r.name, lastSeen: Date.now() },
            },
          };
        }),
      hydrate: (list) =>
        set((s) => {
          const records: Record<string, TrustRecord> = {};
          // Local-only memos (untrusted renames/positions) have no backend
          // row — carry them over so hydration never erases pure-UI state.
          for (const r of Object.values(s.records))
            if (!r.trusted) records[r.deviceId] = r;
          for (const p of list) {
            const prev = s.records[p.deviceId];
            records[p.deviceId] = {
              deviceId: p.deviceId,
              name: p.name,
              // the user's rename is local, never backend state (M7)
              alias: prev?.alias,
              trusted: true,
              autoAccept: p.autoAccept,
              // backend stamps are unix seconds; the UI works in epoch ms
              addedAt: p.pairedAt
                ? p.pairedAt * 1000
                : (prev?.addedAt ?? Date.now()),
              lastSeen: p.lastSeen
                ? p.lastSeen * 1000
                : (prev?.lastSeen ?? Date.now()),
              // circle positions are UI layout, never backend state
              pos: prev?.pos,
            };
          }
          return { records };
        }),
    }),
    {
      name: "lanbeam.trust",
      storage: createJSONStorage(() => safeLocalStorage),
    },
  ),
);

/** True while the one-time legacy import below is running. Each imported
 *  record makes the backend emit `trust_updated` with a partial-so-far list;
 *  hydrating those mid-loop would erase not-yet-imported records (and their
 *  circle positions), so AppShell's listener skips events while this is set
 *  and the loop ends with one authoritative hydrate instead. */
let trustMigrating = false;
export const isTrustMigrating = (): boolean => trustMigrating;

/** One-time legacy import + hydration (Tauri only): push localStorage-era
 *  trust records into the backend store once (guarded by the persisted
 *  `migrated` flag), then adopt `list_trusted` as the source of truth.
 *  `trust_updated` keeps the store in sync afterwards. */
export async function syncTrustFromBackend(): Promise<void> {
  if (!api.isTauri) return;
  const st = useTrust.getState();
  if (!st.migrated) {
    trustMigrating = true;
    let allOk = true;
    try {
      for (const r of Object.values(st.records)) {
        // Untrusted memos have no backend meaning — importing them would
        // GRANT trust, which only an explicit user action may do.
        if (!r.trusted) continue;
        try {
          // Migration only: carry the legacy timestamps (epoch ms → unix s)
          // so "paired at"/"last seen" survive; regular pushTrust calls omit
          // them and the backend ignores them on updates.
          await api.setTrusted(
            r.deviceId,
            r.name,
            r.autoAccept,
            r.addedAt ? Math.floor(r.addedAt / 1000) : undefined,
            r.lastSeen ? Math.floor(r.lastSeen / 1000) : undefined,
          );
        } catch {
          // A rejected invoke is a real failure (malformed/demo ids resolve
          // fine — the backend just ignores them): leave `migrated` unset so
          // the import retries on the next launch.
          allOk = false;
        }
      }
    } finally {
      trustMigrating = false;
    }
    if (!allOk) {
      // Skip the hydrate too — it would erase the records that failed to
      // import, leaving the retry with nothing to push.
      return;
    }
    useTrust.setState({ migrated: true });
  }
  try {
    useTrust.getState().hydrate(await api.listTrusted());
  } catch {
    /* keep the persisted snapshot until the next trust_updated */
  }
}

export function shortFp(deviceId: string): string {
  const hex = deviceId.replace(/[^a-zA-Z0-9]/g, "").toUpperCase();
  return `${hex.slice(0, 4)} · ${hex.slice(4, 8)}`;
}

/** Merge live discoveries with remembered trust records. */
export function trustList(
  devices: DiscoveredDevice[],
  records: Record<string, TrustRecord>,
): TrustDevice[] {
  const liveIds = new Set(devices.map((d) => d.deviceId));
  const out: TrustDevice[] = [];
  for (const d of devices) {
    const r = records[d.deviceId];
    out.push({
      deviceId: d.deviceId,
      name: trustName(r) || d.name,
      trusted: r?.trusted ?? false,
      autoAccept: r?.autoAccept ?? false,
      online: true,
      fp: shortFp(d.deviceId),
      address: d.address,
      pos: r?.pos,
    });
  }
  for (const r of Object.values(records)) {
    if (liveIds.has(r.deviceId)) continue;
    // A live device under a different key is wearing this remembered device's
    // name. That MIGHT be your device with a new key — or it might be nothing at
    // all, because the name is whatever the peer says it is, and the default one
    // ("LanBeam device", or the hostname) is shared by every install nobody has
    // renamed. Shouting 「指纹已变化」 at two machines that simply never got
    // renamed is crying wolf, and the alarm's whole value is that it is rare.
    //
    // So a name only counts as evidence when it is UNAMBIGUOUS:
    const sameName = devices.filter(
      (d) => d.name === r.name && d.deviceId !== r.deviceId,
    );
    const imposter =
      // exactly one live device is wearing it (several ⇒ the name proves nothing)
      sameName.length === 1 &&
      // and that device is not one you already know in its own right. A device
      // you have a record for is not impersonating anybody — it is just another
      // device that shares a name.
      !records[sameName[0].deviceId]
        ? sameName[0]
        : undefined;
    out.push({
      deviceId: r.deviceId,
      name: trustName(r) ?? r.name,
      trusted: r.trusted,
      autoAccept: r.autoAccept,
      online: false,
      fp: shortFp(r.deviceId),
      fpChanged: imposter ? { newDeviceId: imposter.deviceId } : undefined,
      pos: r.pos,
    });
  }
  return out;
}

/** The remembered record a live device may be impersonating: the id of the
 *  trust record whose name it reappeared under with a DIFFERENT key (see
 *  `trustList`'s `fpChanged`), or undefined. Every way of sending to a device
 *  must ask this first — a click does (the devices page opens the fingerprint
 *  alert), and so must a drop (M19). */
export function fpChangedRecordFor(
  deviceId: string,
  devices: DiscoveredDevice[],
  records: Record<string, TrustRecord>,
): string | undefined {
  return trustList(devices, records).find(
    (r) => r.fpChanged?.newDeviceId === deviceId,
  )?.deviceId;
}
