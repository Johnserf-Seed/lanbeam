/* ── backend-owned data ─────────────────────────────────────────────────── */

import { create } from "zustand";
import * as api from "../../bridge/api";
import type {
  DiscoveredDevice,
  MyIdentity,
  NetworkInfo,
  Settings,
} from "../../bridge/api";
import { usePrefs, type Visibility } from "./prefs";

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
        uiZoom: settings.uiZoom,
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
