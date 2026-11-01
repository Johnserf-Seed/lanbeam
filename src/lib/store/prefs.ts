/* ── preferences (persisted UI settings; backend-owned ones live in useData) */

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import { playSound, type SoundKind } from "../sound";
import { safeLocalStorage } from "./persist";

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
  /** backend-hydrated mirror of Settings.uiZoom — the toggle keeps it for instant
   *  UI, load() overwrites it from the settings blob */
  uiZoom: number;
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
      uiZoom: 1,
      set: (p) => set(p),
    }),
    {
      name: "lanbeam.prefs",
      storage: createJSONStorage(() => safeLocalStorage),
    },
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
