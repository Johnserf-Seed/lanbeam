import { afterEach, describe, expect, it } from "vitest";
import { suppressBrowserShortcuts } from "./browserShortcuts";

let stop: (() => void) | null = null;
afterEach(() => {
  stop?.();
  stop = null;
});

/** Dispatch a keydown on window and report whether the default was cancelled. */
function press(
  key: string,
  mods: { ctrl?: boolean; meta?: boolean; shift?: boolean } = {},
): { prevented: boolean; seenByApp: boolean } {
  // An app-level listener on the SAME target, registered in the bubble phase —
  // it must still receive the event (we cancel the default, we don't swallow it).
  let seenByApp = false;
  const spy = () => {
    seenByApp = true;
  };
  window.addEventListener("keydown", spy);
  const e = new KeyboardEvent("keydown", {
    key,
    ctrlKey: !!mods.ctrl,
    metaKey: !!mods.meta,
    shiftKey: !!mods.shift,
    bubbles: true,
    cancelable: true,
  });
  window.dispatchEvent(e);
  window.removeEventListener("keydown", spy);
  return { prevented: e.defaultPrevented, seenByApp };
}

describe("suppressBrowserShortcuts", () => {
  it("swallows the browser chrome that leaks into a packaged desktop app", () => {
    stop = suppressBrowserShortcuts();
    // The one that prompted this: Ctrl+F used to open WebView2's find bar.
    expect(press("f", { ctrl: true }).prevented).toBe(true);
    expect(press("f", { meta: true }).prevented).toBe(true); // macOS
    expect(press("g", { ctrl: true }).prevented).toBe(true); // find next
    expect(press("G", { ctrl: true, shift: true }).prevented).toBe(true); // find prev
    expect(press("F3").prevented).toBe(true); // find next
    expect(press("p", { ctrl: true }).prevented).toBe(true); // print dialog
    expect(press("s", { ctrl: true }).prevented).toBe(true); // "save page as"
    expect(press("o", { ctrl: true }).prevented).toBe(true); // browser open-file
    expect(press("u", { ctrl: true }).prevented).toBe(true); // view-source
  });

  it("NEVER stops propagation — app handlers must still see the key", () => {
    // The settings page's hotkey-capture reads ctrl/meta off every keydown. If
    // this suppressor swallowed the event instead of just cancelling the default,
    // rebinding a shortcut would silently break.
    stop = suppressBrowserShortcuts();
    const hit = press("f", { ctrl: true });
    expect(hit.prevented).toBe(true);
    expect(hit.seenByApp).toBe(true);
  });

  it("leaves text editing alone", () => {
    stop = suppressBrowserShortcuts();
    for (const k of ["a", "c", "v", "x", "z", "y"]) {
      expect(press(k, { ctrl: true }).prevented, `Ctrl+${k}`).toBe(false);
    }
    // Ctrl+Enter sends a quick text — must survive.
    expect(press("Enter", { ctrl: true }).prevented).toBe(false);
    // Plain typing is untouched.
    expect(press("f").prevented).toBe(false);
  });

  it("leaves devtools alone (they only exist in a dev build anyway)", () => {
    stop = suppressBrowserShortcuts();
    expect(press("F12").prevented).toBe(false);
    expect(press("I", { ctrl: true, shift: true }).prevented).toBe(false);
  });

  it("keeps reload in dev, blocks it in the shipped app", () => {
    // Reload is browser chrome too — but taking Ctrl+R from a developer is just
    // hostile, so it survives in dev and only the release build blocks it.
    stop = suppressBrowserShortcuts({ allowReload: true });
    expect(press("r", { ctrl: true }).prevented).toBe(false);
    expect(press("F5").prevented).toBe(false);
    stop();

    stop = suppressBrowserShortcuts({ allowReload: false });
    expect(press("r", { ctrl: true }).prevented).toBe(true);
    expect(press("R", { ctrl: true, shift: true }).prevented).toBe(true); // hard reload
    expect(press("F5").prevented).toBe(true);
  });

  it("cleans up after itself", () => {
    const off = suppressBrowserShortcuts();
    expect(press("f", { ctrl: true }).prevented).toBe(true);
    off();
    expect(press("f", { ctrl: true }).prevented).toBe(false);
  });
});
