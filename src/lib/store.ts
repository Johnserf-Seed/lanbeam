import { create } from "zustand";
import {
  persist,
  type PersistStorage,
  type StorageValue,
} from "zustand/middleware";
import * as api from "../bridge/api";
import type {
  MyIdentity,
  Settings,
  DiscoveredDevice,
  IncomingRequest,
  NetworkInfo,
  TransferErrorCode,
  TrustedPeer,
} from "../bridge/api";
import { baseName, extOf } from "./format";
import type { FileCat } from "./filecat";
import { fileCat } from "./filecat";
import { playSound, type SoundKind } from "./sound";

/* ── preferences (persisted UI settings; backend-owned ones live in useData) */

export type ThemeMode = "light" | "dark" | "system";
export type Visibility = "on" | "ghost" | "off";

interface PrefsState {
  themeMode: ThemeMode;
  /** epoch ms until which the device is temporarily hidden (临时隐身) */
  ghostUntil: number | null;
  recvPolicy: string;
  conflict: string;
  organize: string;
  verifyHash: boolean;
  /** backend-hydrated mirrors (M5.3–5.5): useData.load overwrites them from
   *  the settings blob; the toggles keep them for instant UI only */
  autoStart: boolean;
  trayClose: boolean;
  notifSys: boolean;
  notifSound: boolean;
  soundKind: SoundKind;
  /** backend-hydrated mirror of Settings.hotkeyEnabled (M5.5): the toggle keeps
   *  it for instant UI, load() overwrites it from the settings blob */
  hotkeyEnabled: boolean;
  /** backend-hydrated mirror of Settings.hotkey (M5.5 rebind): the canonical
   *  accelerator ("Alt+Space"); the capture flow updates it, load() overwrites
   *  it from the settings blob. The row derives its display label from this. */
  hotkey: string;
  clipShare: boolean;
  histKeep: string;
  stripExif: boolean;
  logLevel: string;
  /** backend-hydrated mirrors of Settings.port / Settings.ifaceFilter (M5.2/5.6) */
  port: string;
  iface: string;
  /** unused since M5.8 (the visibility select owns discoverability) — kept so
   *  the persisted prefs blob needs no migration; nothing renders it */
  mdns: boolean;
  concurrent: number;
  rate: string;
  ssidOnly: string;
  set: (p: Partial<PrefsState>) => void;
}

export const usePrefs = create<PrefsState>()(
  persist(
    (set) => ({
      themeMode: "light",
      ghostUntil: null,
      recvPolicy: "trusted",
      conflict: "ask",
      organize: "device",
      verifyHash: true,
      autoStart: false,
      trayClose: true,
      notifSys: true,
      notifSound: false,
      soundKind: "叮咚",
      hotkeyEnabled: false,
      hotkey: "Alt+Space",
      clipShare: false,
      histKeep: "30d",
      stripExif: true,
      logLevel: "normal",
      port: "51704",
      iface: "",
      mdns: true,
      concurrent: 3,
      rate: "unlimited",
      ssidOnly: "any",
      set: (p) => set(p),
    }),
    { name: "lanbeam.prefs" },
  ),
);

/** System dark-mode flag, kept fresh by AppShell. */
export const useSysDark = create<{ dark: boolean; set: (v: boolean) => void }>(
  (set) => ({
    dark:
      typeof window !== "undefined" &&
      !!window.matchMedia?.("(prefers-color-scheme: dark)").matches,
    set: (dark) => set({ dark }),
  }),
);

export function resolvedTheme(
  mode: ThemeMode,
  sysDark: boolean,
): "light" | "dark" {
  return mode === "system" ? (sysDark ? "dark" : "light") : mode;
}

/** Play the notification sound if enabled. */
export function notify(): void {
  const p = usePrefs.getState();
  if (p.notifSound) playSound(p.soundKind);
}

/* ── backend-owned data ─────────────────────────────────────────────────── */

interface DataState {
  identity: MyIdentity | null;
  settings: Settings | null;
  devices: DiscoveredDevice[];
  downloadDir: string;
  /** local IPv4 endpoints (M5.1) — the sidebar and the settings page pick the
   *  filtered interface (or the first entry), the iface filter lists them all;
   *  refreshed on SettingsPage mount and window focus (M5.6) so a DHCP renumber
   *  / Wi-Fi switch / NIC hotplug doesn't leave them stale for the session */
  networkInfo: NetworkInfo[];
  /** the port the transfer listener is actually bound to right now (M5.2) —
   *  differs from settings.port until a restart, and reflects an ephemeral
   *  fallback if the configured port was taken. 0 until the first load(). */
  listenPort: number;
  /** deviceId → epoch ms first seen this session (drives「刚刚发现」) */
  firstSeen: Record<string, number>;
  load: () => Promise<void>;
  /** re-enumerate local interfaces without a full reload (keeps the previous
   *  list if the call fails, so a transient error never blanks the IP). */
  refreshNetworkInfo: () => Promise<void>;
  setDevices: (d: DiscoveredDevice[]) => void;
  setDeviceName: (name: string) => Promise<void>;
  setDiscoverable: (v: boolean) => Promise<void>;
  setAutoOpen: (v: boolean) => Promise<void>;
  setDownloadDir: (path: string) => Promise<void>;
}

export const useData = create<DataState>((set, get) => ({
  identity: null,
  settings: null,
  devices: [],
  downloadDir: "",
  networkInfo: [],
  listenPort: 0,
  firstSeen: {},
  setDevices: (devices) =>
    set((s) => {
      const firstSeen = { ...s.firstSeen };
      const now = Date.now();
      for (const d of devices)
        if (!firstSeen[d.deviceId]) firstSeen[d.deviceId] = now;
      return { devices, firstSeen };
    }),
  load: async () => {
    const [identity, settings, devices, downloadDir, networkInfo, listenPort] =
      await Promise.all([
        api.getMyIdentity(),
        api.getSettings(),
        api.listDiscoveredDevices(),
        api.getDownloadDir(),
        api.getNetworkInfo().catch(() => [] as NetworkInfo[]),
        api.getListenPort().catch(() => 0),
      ]);
    get().setDevices(devices);
    set({ identity, settings, downloadDir, networkInfo, listenPort });
    // recvPolicy/logLevel/tray/notif/autostart/port/iface/verifyHash live in the
    // backend settings blob (M4.4/4.6/M5/M6.3); the prefs copies are only a
    // mirror for instant UI, so overwrite them on every load. Browser mode keeps
    // its persisted prefs (no backend truth).
    if (api.isTauri) {
      usePrefs.getState().set({
        recvPolicy: settings.recvPolicy,
        logLevel: settings.logLevel,
        trayClose: settings.trayClose,
        notifSys: settings.notifSystem,
        autoStart: settings.autostart,
        hotkeyEnabled: settings.hotkeyEnabled,
        hotkey: settings.hotkey,
        verifyHash: settings.verifyHash,
        // M6.5/6.6/6.7: the conflict/organize/concurrency/rate prefs are now
        // backed by the settings blob — overwrite the mirrors like recvPolicy.
        conflict: settings.conflict,
        organize: settings.organize,
        concurrent: settings.maxConcurrent,
        rate: settings.rateLimit,
        // M7.3: the clipboard-sharing consent is backend-owned too.
        clipShare: settings.clipShare,
        // M9.1: the strip-metadata default is backend-owned; the mirror still
        // seeds the confirm-modal checkbox for instant UI.
        stripExif: settings.stripExif,
        // 0 is the "use the default" sentinel — show the port actually used
        port: String(settings.port || 51704),
        iface: settings.ifaceFilter ?? "",
      });
    }
  },
  refreshNetworkInfo: async () => {
    // Degrade to the previous list on failure (null → skip the set) rather
    // than blanking the sidebar / iface select on a transient enumeration error.
    const info = await api.getNetworkInfo().catch(() => null);
    if (info) set({ networkInfo: info });
  },
  setDeviceName: async (name) => {
    await api.setDeviceName(name);
    const [identity, settings] = await Promise.all([
      api.getMyIdentity(),
      api.getSettings(),
    ]);
    set({ identity, settings });
  },
  setDiscoverable: async (v) => {
    // Optimistic: flip the flag before the IPC round-trip so the visibility
    // select/pill doesn't snap back to the old value; roll back on failure.
    const prev = get().settings;
    set({ settings: prev && { ...prev, discoverable: v } });
    try {
      await api.setDiscoverable(v);
    } catch (e) {
      set({ settings: prev });
      throw e;
    }
  },
  setAutoOpen: async (v) => {
    await api.setAutoOpen(v);
    const s = get().settings;
    set({ settings: s && { ...s, autoOpenFolder: v } });
  },
  setDownloadDir: async (path) => {
    // The backend canonicalizes (and can reject a vanished folder) — adopt
    // ITS path, not the picker's raw one, so the row shows what receiving
    // actually uses (M5.2).
    const stored = await api.setDownloadDir(path);
    const s = get().settings;
    set({
      downloadDir: stored,
      settings: s && { ...s, downloadDirOverride: stored },
    });
  },
}));

/** The IP to show in the sidebar / identity card: honor the user's interface
 *  filter when it matches a live interface, else the first entry — mirroring
 *  the backend's own fall-back to all interfaces when the stored filter no
 *  longer resolves (M5.6). Without this, a numeric-first list surfaces a
 *  VPN/Hyper-V/WSL address LAN peers can't reach. */
export function displayIp(
  info: NetworkInfo[],
  iface: string,
): string | undefined {
  return (iface && info.find((n) => n.ip === iface)?.ip) || info[0]?.ip;
}

/** Current visibility tri-state derived from backend flag + ghost timer. */
export function visibilityOf(
  settings: Settings | null,
  ghostUntil: number | null,
  now = Date.now(),
): Visibility {
  if (settings?.discoverable) return "on";
  if (ghostUntil && ghostUntil > now) return "ghost";
  return "off";
}

/** Switch visibility; "ghost" hides now and auto-restores after 1 h.
 *  Applies optimistically; rolls back and rethrows if the backend rejects,
 *  so callers can gate success feedback on the resolved promise. */
export async function setVisibility(v: Visibility): Promise<void> {
  const data = useData.getState();
  const prevGhost = usePrefs.getState().ghostUntil;
  usePrefs
    .getState()
    .set({ ghostUntil: v === "ghost" ? Date.now() + 3600_000 : null });
  try {
    await data.setDiscoverable(v === "on");
  } catch (e) {
    usePrefs.getState().set({ ghostUntil: prevGhost });
    throw e;
  }
}

/* ── trust circle ───────────────────────────────────────────────────────── */

export type TrustRecord = {
  deviceId: string;
  name: string;
  trusted: boolean;
  autoAccept: boolean;
  addedAt: number;
  lastSeen: number;
  /** free-mode position on the 640×430 base canvas */
  pos?: { x: number; y: number };
};

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
  migrate: (oldId: string, newId: string, name: string) => void;
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
          trusted,
          autoAccept: trusted ? (prev?.autoAccept ?? false) : false,
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
      remove: (id) => {
        set((s) => {
          const records = { ...s.records };
          delete records[id];
          return { records, sel: s.sel === id ? null : s.sel };
        });
        dropTrust(id);
      },
      restore: (rec) => {
        set((s) => ({ records: { ...s.records, [rec.deviceId]: rec } }));
        if (rec.trusted) pushTrust(rec.deviceId, rec.name, rec.autoAccept);
      },
      rename: (id, name) => {
        const prev = get().records[id];
        const rec: TrustRecord = prev
          ? { ...prev, name }
          : {
              deviceId: id,
              name,
              trusted: false,
              autoAccept: false,
              addedAt: Date.now(),
              lastSeen: Date.now(),
            };
        set((s) => ({ records: { ...s.records, [id]: rec } }));
        // rename = set_trusted with the new name; untrusted memos stay local.
        if (rec.trusted) pushTrust(id, name, rec.autoAccept);
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
      migrate: (oldId, newId, name) => {
        const old = get().records[oldId];
        const rec: TrustRecord = {
          deviceId: newId,
          name: name || old?.name || "",
          trusted: true,
          autoAccept: old?.autoAccept ?? false,
          addedAt: old?.addedAt ?? Date.now(),
          lastSeen: Date.now(),
          pos: old?.pos,
        };
        set((s) => {
          const records = { ...s.records };
          delete records[oldId];
          records[newId] = rec;
          return { records, sel: newId };
        });
        dropTrust(oldId);
        pushTrust(newId, rec.name, rec.autoAccept);
      },
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
    { name: "lanbeam.trust" },
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
      name: r?.name || d.name,
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
    // Same display name showed up under a different key → fingerprint change.
    const imposter = devices.find(
      (d) => d.name === r.name && d.deviceId !== r.deviceId,
    );
    out.push({
      deviceId: r.deviceId,
      name: r.name,
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

/* ── transfers ──────────────────────────────────────────────────────────── */

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
  /** transfer_started arrived (peer accepted / channel open) */
  started?: boolean;
  peerId?: string;
  peerName?: string;
  /** display name: first file (+ n more handled by the page) */
  name?: string;
  ext?: string;
  files?: { name: string; size?: number }[];
  /** outgoing source paths (再次发送) */
  paths?: string[];
  /** paused via pause_transfer (M6.2). Session-local backpressure — the backend
   *  emits no state event, so the UI owns this flag and clears it on resume. */
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
  /** deviceId → FIFO queue of metas for sends we just initiated (each consumed
   *  by the matching sas_code). A per-device queue (not a single slot) so a
   *  second send to the same device before the first's sas_code arrives — easy
   *  when the first parks on the concurrency gate — can't clobber the first. */
  pendingSend: Record<string, OutgoingMeta[]>;
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
  registerOutgoing: (deviceId: string, meta: OutgoingMeta) => void;
  attachSas: (sessionId: string, sas: string, deviceId?: string) => void;
  pushIncoming: (r: IncomingRequest) => void;
  shiftIncoming: () => void;
  /** drop a specific prompt card (e.g. its session already errored out) */
  removeIncoming: (sessionId: string) => void;
  acceptMeta: (r: IncomingRequest, peerName: string) => void;
  removeTransfer: (sessionId: string) => void;
}

// Monotonic counter for quick-text record ids. Combined with a timestamp and a
// random suffix under a distinctive "text-" prefix, it guarantees a text
// record's sessionId can never collide with a backend transfer_id (a UUID) or
// with another text record minted in the same millisecond.
let textRecordSeq = 0;

/** Persisted shape of useTransfers (see partialize below): terminal rows only. */
type PersistedTransfers = { transfers: Record<string, UITransfer> };

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
      const str = localStorage.getItem(name);
      return str ? (JSON.parse(str) as StorageValue<PersistedTransfers>) : null;
    },
    setItem: (name, value) => {
      const records = Object.values(value.state.transfers);
      if (
        lastRecords &&
        lastRecords.length === records.length &&
        records.every((r, i) => r === lastRecords?.[i])
      ) {
        return; // nothing terminal changed — skip serialize + write
      }
      lastRecords = records;
      localStorage.setItem(name, JSON.stringify(value));
    },
    removeItem: (name) => localStorage.removeItem(name),
  };
}

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
          return { transfers: { ...s.transfers, [t.sessionId]: merged } };
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
          if (prev._tm && prev._pm !== undefined && percent > prev._pm) {
            const dBytes = (total * (percent - prev._pm)) / 100;
            const dt = Math.max(now - prev._tm, 1) / 1000;
            speedBps = dBytes / dt;
            hist = [...hist.slice(-39), speedBps / 1048576];
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
                _pm: percent,
                _tm: now,
              },
            },
          };
        }),
      setPaused: (sessionId, paused) =>
        set((s) => {
          const prev = s.transfers[sessionId];
          if (!prev) return s;
          return {
            transfers: { ...s.transfers, [sessionId]: { ...prev, paused } },
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
      registerOutgoing: (deviceId, meta) =>
        set((s) => ({
          pendingSend: {
            ...s.pendingSend,
            [deviceId]: [...(s.pendingSend[deviceId] ?? []), meta],
          },
        })),
      attachSas: (sessionId, sas, deviceId) =>
        set((s) => {
          const queue = deviceId ? s.pendingSend[deviceId] : undefined;
          const meta = queue?.[0];
          // Consume the OLDEST pending meta (FIFO) so a later stray sas_code for
          // the same device can't attach stale file info to an unrelated
          // session; drop the key once its queue empties.
          let pendingSend = s.pendingSend;
          if (queue && deviceId) {
            const rest = queue.slice(1);
            pendingSend = { ...pendingSend };
            if (rest.length) pendingSend[deviceId] = rest;
            else delete pendingSend[deviceId];
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
                ...(meta
                  ? {
                      peerName: meta.peerName,
                      name: meta.name,
                      ext: meta.ext,
                      files: meta.files,
                      paths: meta.paths,
                    }
                  : {}),
              },
            },
          };
        }),
      pushIncoming: (r) => set((s) => ({ incomings: [...s.incomings, r] })),
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
      partialize: (s) => ({
        transfers: Object.fromEntries(
          Object.entries(s.transfers)
            .filter(([, t]) => t.status === "done" || t.status === "error")
            .sort((a, b) => (b[1].doneAt ?? 0) - (a[1].doneAt ?? 0))
            .slice(0, 200),
        ),
      }),
    },
  ),
);

/** Ordered list helpers. */
export function transferList(map: Record<string, UITransfer>): UITransfer[] {
  return Object.values(map).sort((a, b) => b.startedAt - a.startedAt);
}

/* ── inbox ──────────────────────────────────────────────────────────────── */

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
    { name: "lanbeam.inbox" },
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

/* ── recents (files offered in the send flow) ───────────────────────────── */

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
    { name: "lanbeam.recents" },
  ),
);

/** SendFile from an absolute path. */
export function sendFileFromPath(p: string): SendFile {
  const name = baseName(p);
  return { path: p, name, ext: extOf(name) };
}

/* ── toast ──────────────────────────────────────────────────────────────── */

type ToastAction = { label: string; fn: () => void };

interface ToastState {
  msg: string | null;
  action: ToastAction | null;
  show: (msg: string, action?: ToastAction | null, ms?: number) => void;
  hide: () => void;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;

export const useToast = create<ToastState>((set) => ({
  msg: null,
  action: null,
  show: (msg, action, ms) => {
    clearTimeout(toastTimer);
    set({ msg, action: action ?? null });
    toastTimer = setTimeout(() => set({ msg: null, action: null }), ms ?? 3200);
  },
  hide: () => {
    clearTimeout(toastTimer);
    set({ msg: null, action: null });
  },
}));

export const showToast = (
  msg: string,
  action?: ToastAction | null,
  ms?: number,
): void => useToast.getState().show(msg, action, ms);

/* ── overlays (send flow, drawers, modals) ──────────────────────────────── */

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

export type FpAlert = {
  deviceId: string;
  step: "warn" | "verify";
  sas?: string;
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
  detailId: string | null;
  pairOpen: boolean;
  /** A `lanbeam://pair` link captured from a deep link, staged for the pairing
   *  modal to pre-fill its join field. Cleared once consumed / on close. */
  pairPrefill: string | null;
  qtOpen: boolean;
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
  closeSend: () => void;
  setDetail: (id: string | null) => void;
  setPair: (v: boolean, prefill?: string | null) => void;
  setPairPrefill: (v: string | null) => void;
  setQt: (v: boolean) => void;
  setShare: (v: boolean) => void;
  setLicense: (v: boolean) => void;
  setFpAlert: (v: FpAlert | null) => void;
  setConflict: (v: PendingConflict | null) => void;
  setDrag: (over: boolean, device?: string | null) => void;
}

export const useOverlays = create<OverlayState>((set) => ({
  send: null,
  detailId: null,
  pairOpen: false,
  pairPrefill: null,
  qtOpen: false,
  shareOpen: false,
  licenseOpen: false,
  fpAlert: null,
  conflict: null,
  dragOver: false,
  dragDevice: null,
  openSend: (deviceId, pool, presel) =>
    set({
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
    }),
  patchSend: (p) => set((s) => (s.send ? { send: { ...s.send, ...p } } : s)),
  closeSend: () => set({ send: null }),
  setDetail: (detailId) => set({ detailId }),
  // Opening with a prefill stashes the pairing link for the modal to consume;
  // closing (or opening without one) clears it so a later manual open is clean.
  setPair: (pairOpen, prefill = null) =>
    set({ pairOpen, pairPrefill: pairOpen ? prefill : null }),
  setPairPrefill: (pairPrefill) => set({ pairPrefill }),
  setQt: (qtOpen) => set({ qtOpen }),
  setShare: (shareOpen) => set({ shareOpen }),
  setLicense: (licenseOpen) => set({ licenseOpen }),
  setFpAlert: (fpAlert) => set({ fpAlert }),
  setConflict: (conflict) => set({ conflict }),
  setDrag: (dragOver, dragDevice = null) => set({ dragOver, dragDevice }),
}));
