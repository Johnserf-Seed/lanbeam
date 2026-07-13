// Component tests for WindowControls — the frontend-drawn window chrome
// (─ □ ×) for the undecorated Tauri window. The component renders nothing in a
// plain browser, so we force the Tauri branch by mocking api.isTauri = true.
// The component dynamically imports @tauri-apps/api/window and calls the real
// Window instance methods; we spy on Window.prototype (and stub the Tauri
// internals global) so the min/max/close clicks are observable without IPC.
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as tauriWindow from "@tauri-apps/api/window";
import i18n from "../i18n";
import { fireEvent, renderUI, screen, waitFor } from "../test/render";
import WindowControls from "./WindowControls";

vi.mock("../bridge/api", () => ({ isTauri: true }));

// getCurrentWindow() reads window.__TAURI_INTERNALS__.metadata; provide a stub
// so the real Window instance can be constructed in happy-dom.
(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "main" } },
};

const WinProto = tauriWindow.Window.prototype;

let minimize: ReturnType<typeof vi.spyOn>;
let toggleMaximize: ReturnType<typeof vi.spyOn>;
let close: ReturnType<typeof vi.spyOn>;

describe("WindowControls", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    // useEffect probes isMaximized() + subscribes onResized(); keep them inert.
    vi.spyOn(WinProto, "isMaximized").mockResolvedValue(false);
    vi.spyOn(WinProto, "onResized").mockResolvedValue(() => {});
    minimize = vi.spyOn(WinProto, "minimize").mockResolvedValue(undefined);
    toggleMaximize = vi
      .spyOn(WinProto, "toggleMaximize")
      .mockResolvedValue(undefined);
    close = vi.spyOn(WinProto, "close").mockResolvedValue(undefined);
  });

  it("renders the three window-control buttons", () => {
    renderUI(<WindowControls />);
    expect(screen.getByTitle(i18n.t("win.minimize"))).toBeInTheDocument();
    expect(screen.getByTitle(i18n.t("win.maximize"))).toBeInTheDocument();
    expect(screen.getByTitle(i18n.t("win.close"))).toBeInTheDocument();
  });

  it("minimize button calls window.minimize()", async () => {
    renderUI(<WindowControls />);
    fireEvent.click(screen.getByTitle(i18n.t("win.minimize")));
    await waitFor(() => expect(minimize).toHaveBeenCalledTimes(1));
  });

  it("maximize button calls window.toggleMaximize()", async () => {
    renderUI(<WindowControls />);
    fireEvent.click(screen.getByTitle(i18n.t("win.maximize")));
    await waitFor(() => expect(toggleMaximize).toHaveBeenCalledTimes(1));
  });

  it("close button calls window.close()", async () => {
    renderUI(<WindowControls />);
    fireEvent.click(screen.getByTitle(i18n.t("win.close")));
    await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  });
});
