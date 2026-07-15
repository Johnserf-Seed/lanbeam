import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../bridge/api";
import { usePrefs } from "./store";
import { installZoomHotkeys, setZoom, stepZoom, UI_ZOOMS } from "./uiZoom";

vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return { ...actual, setUiZoom: vi.fn(() => Promise.resolve()) };
});

beforeEach(() => {
  usePrefs.getState().set({ uiZoom: 1 });
  vi.mocked(api.setUiZoom).mockClear();
});

describe("stepZoom", () => {
  it("walks the ladder one rung at a time", () => {
    expect(stepZoom(1, 1)).toBe(1.1);
    expect(stepZoom(1.1, 1)).toBe(1.25);
    expect(stepZoom(1, -1)).toBe(0.9);
  });

  it("stops at the ends rather than wrapping or running off", () => {
    const [min] = UI_ZOOMS;
    const max = UI_ZOOMS[UI_ZOOMS.length - 1];
    expect(stepZoom(min, -1)).toBe(min);
    expect(stepZoom(max, 1)).toBe(max);
  });

  it("snaps a value that isn't on the ladder to the nearest rung first", () => {
    // A hand-edited settings.json (or a rung a later build dropped) must not make
    // a step a no-op — the user pressed Ctrl+ and something has to move.
    expect(stepZoom(1.03, 1)).toBe(1.1); // nearest rung is 1 → up to 1.1
    expect(stepZoom(1.03, -1)).toBe(0.9); // nearest rung is 1 → down to 0.9
  });
});

describe("界面缩放 hotkeys", () => {
  const press = (key: string, mod = true) =>
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key, ctrlKey: mod, bubbles: true }),
    );

  it("binds Ctrl +/-/0 to the APP's scale, not the browser's", async () => {
    // The webview's own zoom hotkeys are off — browser chrome doesn't belong in a
    // packaged app. These keys drive the persisted setting instead.
    const off = installZoomHotkeys();

    press("=");
    await vi.waitFor(() => expect(api.setUiZoom).toHaveBeenCalledWith(1.1));

    usePrefs.getState().set({ uiZoom: 1.1 });
    press("-");
    await vi.waitFor(() => expect(api.setUiZoom).toHaveBeenCalledWith(1));

    usePrefs.getState().set({ uiZoom: 1.5 });
    press("0");
    await vi.waitFor(() => expect(api.setUiZoom).toHaveBeenCalledWith(1));

    off();
  });

  it("ignores the same keys without a modifier", () => {
    const off = installZoomHotkeys();
    press("=", false);
    press("0", false);
    expect(api.setUiZoom).not.toHaveBeenCalled();
    off();
  });

  it("stops listening once torn down", () => {
    installZoomHotkeys()();
    press("=");
    expect(api.setUiZoom).not.toHaveBeenCalled();
  });
});

describe("setZoom", () => {
  it("rolls the mirror back when the backend refuses", async () => {
    vi.mocked(api.setUiZoom).mockRejectedValueOnce({ kind: "Io" });
    await setZoom(1.5);
    // Never leave the dropdown claiming a scale the window never took.
    expect(usePrefs.getState().uiZoom).toBe(1);
  });

  it("does nothing when the scale is already the one asked for", async () => {
    await setZoom(1);
    expect(api.setUiZoom).not.toHaveBeenCalled();
  });
});
