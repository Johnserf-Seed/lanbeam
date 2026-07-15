/** 界面缩放 — the app's own zoom, not the browser's.
 *
 *  The webview's native zoom hotkeys are OFF (Tauri's `zoomHotkeysEnabled` defaults
 *  to false, which kills WebView2's `IsZoomControlEnabled`), which is right: browser
 *  chrome doesn't belong in a packaged app. But a desktop app WITH a scale setting
 *  and WITHOUT Ctrl +/-/0 is just missing its front door. So these keys drive the
 *  app's own setting instead — the same one the settings page writes, persisted, and
 *  applied by the backend.
 *
 *  The backend does two things with it, and the second is the one that is easy to
 *  miss: it zooms the webview, AND it raises the window's minimum size to match. A
 *  zoom SHRINKS the CSS viewport — a 920pt-wide window at 150% leaves the layout 613
 *  CSS px, well under the 920 it needs — so a floor that ignored the zoom would let
 *  you scale the interface straight off the edge of its own window.
 */
import * as api from "../bridge/api";
import { errText } from "./sendops";
import { showToast, useData, usePrefs } from "./store";

/** The offered scales. The backend clamps to the same [0.8, 1.5]; keeping the ladder
 *  in ONE place is what stops the settings dropdown and the hotkeys from drifting
 *  onto different rungs. */
export const UI_ZOOMS = [0.8, 0.9, 1, 1.1, 1.25, 1.5] as const;

/** The next rung up (`+1`) or down (`-1`) from `current`, clamped at the ends. A
 *  value that isn't on the ladder (a hand-edited settings.json) snaps to the nearest
 *  rung first, so a step is never a no-op. */
export function stepZoom(current: number, dir: 1 | -1): number {
  let nearest = 0;
  for (let i = 1; i < UI_ZOOMS.length; i++) {
    if (
      Math.abs(UI_ZOOMS[i] - current) < Math.abs(UI_ZOOMS[nearest] - current)
    ) {
      nearest = i;
    }
  }
  const next = Math.min(Math.max(nearest + dir, 0), UI_ZOOMS.length - 1);
  return UI_ZOOMS[next];
}

/** Commit a scale: mirror it for an instant UI, then let the backend do the real
 *  work (webview zoom + the matching window floor) and persist it. Rolls the mirror
 *  back if the backend refuses, rather than leaving the dropdown claiming a scale
 *  the window never took. */
export async function setZoom(z: number): Promise<void> {
  const prev = usePrefs.getState().uiZoom;
  if (z === prev) return;
  usePrefs.getState().set({ uiZoom: z });
  try {
    await api.setUiZoom(z);
    useData.setState((s) => ({
      settings: s.settings && { ...s.settings, uiZoom: z },
    }));
  } catch (e) {
    usePrefs.getState().set({ uiZoom: prev });
    showToast(errText(e));
  }
}

/** Ctrl/Cmd with `+` / `-` / `0`. Returns the cleanup. */
export function installZoomHotkeys(): () => void {
  const onKeyDown = (e: KeyboardEvent) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const cur = usePrefs.getState().uiZoom;
    switch (e.key) {
      // "=" is the unshifted key; "+" is it with Shift, and the numpad's own.
      case "=":
      case "+":
        e.preventDefault();
        void setZoom(stepZoom(cur, 1));
        break;
      case "-":
      case "_":
        e.preventDefault();
        void setZoom(stepZoom(cur, -1));
        break;
      case "0":
        e.preventDefault();
        void setZoom(1);
        break;
    }
  };
  window.addEventListener("keydown", onKeyDown, true);
  return () => window.removeEventListener("keydown", onKeyDown, true);
}
