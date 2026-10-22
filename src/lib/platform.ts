/** True on macOS, for the few places the UI has to name keys the way the OS
 *  does (⌘ ⌥ ⇧ there, Ctrl / Alt / Shift everywhere else). The webview reports
 *  its host: WKWebView says "MacIntel" (on Apple Silicon too), WebView2 "Win32",
 *  WebKitGTK "Linux …". */
export function isMac(): boolean {
  const ua =
    (typeof navigator !== "undefined" &&
      (navigator.platform || navigator.userAgent)) ||
    "";
  return /Mac/i.test(ua);
}
