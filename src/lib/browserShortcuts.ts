/** Take the BROWSER out of the WebView.
 *
 *  WebView2 ships Chromium's whole keyboard surface: Ctrl+F opens a find bar,
 *  Ctrl+P a print dialog, Ctrl+S a "save page as", Ctrl+U a view-source tab. In a
 *  packaged desktop app none of those are features — they are leaks. The user
 *  never asked for browser chrome, and not one of them does anything useful here.
 *
 *  WHY NOT JUST TURN THEM OFF: WebView2 can kill the lot with a single flag
 *  (`AreBrowserAcceleratorKeysEnabled`), and wry exposes it
 *  (`WebViewBuilderExtWindows::with_browser_accelerator_keys`) — but Tauri 2.11
 *  does NOT pass it through, so there is no config or builder switch to flip. The
 *  webview-level fallback is to swallow the keydown: Chromium hands these keys to
 *  the page BEFORE acting on them, which is exactly how Docs/Notion take Ctrl+F
 *  for themselves.
 *
 *  We only ever `preventDefault`, NEVER `stopPropagation` — the app's own
 *  handlers (the settings page's hotkey-capture reads ctrl/meta on every keydown)
 *  must still see the event. Cancelling the browser's default action is the whole
 *  job; swallowing the event outright would break real features.
 *
 *  DELIBERATELY LEFT ALONE:
 *  - text editing (Ctrl+A/C/V/X/Z) — obviously;
 *  - F12 / Ctrl+Shift+I (devtools) — devtools are only compiled into a DEV build
 *    (the `devtools` Cargo feature is off, and it is not one of tauri's defaults),
 *    so a release has nothing to hide and a developer wants them in dev;
 *  - zoom (Ctrl +/-/0, Ctrl+wheel) — the WEBVIEW's zoom is off (Tauri's
 *    `zoomHotkeysEnabled` defaults to false, killing WebView2's
 *    `IsZoomControlEnabled`), and `lib/uiZoom.ts` binds those same keys to the app's
 *    own 界面缩放 setting instead. Zooming a desktop app is a feature; zooming a web
 *    page inside one is a leak. Same keys, different owner.
 */

/** Browser chrome reachable with Ctrl/Cmd, keyed by lowercased `event.key`. */
const WITH_MOD = ["f", "g", "p", "s", "o", "u"] as const;
/** Bare keys that drive browser chrome (find-next, reload). */
const BARE = ["F3"] as const;

export type SuppressOpts = {
  /** Keep Ctrl+R / F5 working. Reload is browser chrome too, so the SHIPPED app
   *  blocks it — but a dev build is a developer's workspace and taking reload
   *  away there is just hostile. Defaults to "allow in dev, block in prod". */
  allowReload?: boolean;
};

/** Install the suppressor on `window`. Returns the cleanup. */
export function suppressBrowserShortcuts(opts: SuppressOpts = {}): () => void {
  const allowReload = opts.allowReload ?? import.meta.env.DEV;
  const withMod = new Set<string>(WITH_MOD);
  const bare = new Set<string>(BARE);
  if (!allowReload) {
    withMod.add("r"); // Ctrl+R, and Ctrl+Shift+R (event.key is "R")
    bare.add("F5");
  }

  const onKeyDown = (e: KeyboardEvent) => {
    if (bare.has(e.key)) {
      e.preventDefault();
      return;
    }
    if (!(e.ctrlKey || e.metaKey)) return;
    if (withMod.has(e.key.toLowerCase())) e.preventDefault();
  };

  // Capture phase so we get there before anything else could act on it — but,
  // again, we never stop propagation, so app handlers still receive the key.
  window.addEventListener("keydown", onKeyDown, true);
  return () => window.removeEventListener("keydown", onKeyDown, true);
}
