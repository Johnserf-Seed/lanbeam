/* ── toast ──────────────────────────────────────────────────────────────── */

import { create } from "zustand";

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
